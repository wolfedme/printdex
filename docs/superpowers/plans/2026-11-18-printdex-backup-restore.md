# Printdex Backup & Restore (Milestone 5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Printdex backs up its own data (always) and model files (per library) to a folder, S3, WebDAV or SFTP target as deduplicated, compressed, optionally encrypted snapshots, runs unattended while the app is open, shows backup health, and can rebuild a library on a new machine or bring back a single model.

**Architecture:**
- **Part A, engine** (`printdex-core/src/backup/`):
  - `repo.rs` wraps `rustic_core`/`rustic_backend`: open or init a repository for a target, back up a path set with tags, list snapshots, restore a snapshot subtree, forget/prune, check. Its API is synchronous, so every job runs on a blocking thread with a cancel flag and a progress channel.
  - `config.rs` holds the target (shaped as a list), encryption choice, schedules and per-library model switches in the database. Secrets go to the OS credential store through `secrets.rs` (`keyring`), never into the DB, settings or a backup.
  - `data.rs` builds the data snapshot: `VACUUM INTO` a temp copy of the DB, plus attempt photos and the settings files.
  - `scheduler.rs` owns timers: data 5 min after the last change and at least daily; models on their schedule; due-on-launch; retention, weekly prune and the monthly sample check; and works out "needs attention".
- **Part B, app:** Settings › Backup (target form with Test connection, encryption, model switches with size estimate and a confirm step, schedules, Back up now, history), a status line in the sidebar footer, and passphrase prompts.
- **Part C, restore:**
  - Engine `restore.rs`: data restore into a staging folder, swapped in on the next start with a **new** install id that records its predecessor. Library re-linking, and rewriting sidecars from the restored DB.
  - App: "Restore from backup" on first launch and in Settings, a per-library "where does it live now" step, and single-model restore from the model view and the Missing banner. Existing files go to the OS trash first.

**Tech Stack:**
- `rustic_core` 0.14 and `rustic_backend` 0.8, with only the `opendal` feature and the `fs`, `s3`, `webdav` and (non-Windows) `sftp` services.
- `keyring` 4 (Keychain, Windows Credential Manager, Secret Service).
- The rest is existing: tokio, sqlx/SQLite, walk::Rules, sidecars.

**Spec:** `docs/superpowers/specs/2026-11-16-printdex-backup-design.md` (approved). Also v1 §6–§8 (sidecars, missing models, file rules) and architecture §5 (engine commands and events). Epic #21; issues #92–#99 (Tasks 1–8), follow-up #100 (bandwidth limit).

## Decisions made while planning (Claude's calls, flagged for review)

1. **What a model backup reads.** rustic walks the library root itself, with excludes generated from `walk::Rules` (junk folders, OS files, Printdex's own temp files). A test asserts that the backed-up file set equals the scanner's file set for the same tree, so backup and library always agree (spec §5).
2. **Data snapshot layout.** The snapshot root is a staging folder with `printdex.db` (from `VACUUM INTO`), `photos/`, `settings.toml`, `apps.toml` and `backup.toml` (config without secrets). It is tagged `printdex-data`, with host = install id. The staging folder is deleted after the run.
3. **Change signal for the data debounce.** Engine events that change user data (tags, notes, recipe, attempts, collections, library add/remove/rename) bump a `data_changed_at` timestamp. The scheduler backs up 5 min after the last bump.
4. **Repository password.** Encryption off means the empty string (spec §7). A round-trip test pins that rustic accepts it (spec §13).
5. **Credential store missing.** `secrets::Store` falls back to "ask each time": the scheduler pauses and raises `BackupNeedsAttention::PassphraseNeeded` or `CredentialsNeeded`, and the app shows a prompt that keeps the secret only in memory for the session.
6. **Restore swap.** A data restore writes to `data/restore-staging/` and a marker file. On the next `Engine::open`, before the DB is opened, the marker swaps the staging DB and photos in (the old ones go to `data/pre-restore-<time>/`, kept until the next successful backup). The app restarts itself after a data restore.
7. **Single-model restore target.** By default it restores into the model's folder. Existing files whose content differs are moved to the OS trash first, and identical files are left alone. "Restore to another folder" is the alternative. The model's sidecar comes back and is imported.
8. **Bandwidth limit.** Deferred: rustic_core has no upload rate limit, and a wrapper backend is more code than it is worth for the MVP. Low priority comes from a single backup thread plus yielding while a scan runs. Recorded as a follow-up issue.
9. **SFTP on Windows** is hidden in the target list (spec §13), and the `sftp` service is compiled only for `cfg(unix)`.
10. **PR split:** part A (Tasks 1–5), part B (Task 6), part C (Tasks 7–8).

## Global Constraints

- No `unsafe` in Printdex code. Dependencies may use it.
- Never write secrets to the DB, settings, logs or a backup.
- Never upload anything without a configured target. The first model backup needs the confirm step with sizes.
- Restores never overwrite or delete user files in place: they trash first, or restore elsewhere.
- Backup work never blocks the UI thread or delays quitting by more than ~2 s.
- Dependencies stay GPL-3.0-compatible (`cargo-deny`). Trim OpenDAL to the four services.
- Fluent for every new string; tokens only from `theme`. PR CI stays Linux-only.

## Review Focus

1. **The target goes away mid-backup** (USB pulled, Wi-Fi drops). Expect no corrupt repository, the next run resumes, and the status says what happened only once it persists. Test: Task 3 `interrupted_backup_resumes`.
2. **Wrong passphrase or rejected credentials.** Expect a clear "needs attention", no retry storm and nothing written. Test: Task 2 `wrong_passphrase_needs_attention`.
3. **Restoring an older point in time while newer sidecars exist on disk.** The restored data must win and the sidecars must be rewritten. Test: Task 7 `restore_beats_newer_sidecars`.
4. **Two installs sharing one repository.** Each one's thinning never forgets the other's snapshots. Test: Task 5 `retention_is_per_host_and_tag`.
5. **A library offline during a model backup.** It is skipped, the data backup still succeeds, and the 14-day "connect your drive" message appears only after the threshold. Test: Task 5 `offline_library_skipped_not_failed`.

---

## Part A — Backup engine

### Task 1: rustic integration spike and repository wrapper (#92)

**Files:** `crates/printdex-core/Cargo.toml`, `src/backup/mod.rs`, `src/backup/repo.rs`, `tests/backup_repo.rs`, `deny.toml` (if needed)

**Interfaces:**
- Produces:
  - `pub enum Target { Folder { path }, S3 { endpoint, bucket, region, key_id }, WebDav { url, user }, Sftp { host, port, user, key_file } }`
  - `pub struct TargetSecrets { secret: Option<String>, passphrase: String }`
  - `pub struct Repo` with:
    - `open_or_init(&Target, &TargetSecrets) -> Result<Repo>`
    - `backup(paths, excludes, tags, host, cancel, progress) -> Result<SnapshotSummary>`
    - `snapshots(filter) -> Result<Vec<SnapshotInfo>>`
    - `restore(snapshot, subpath, dest, cancel, progress) -> Result<()>`
    - `forget(policy, host, tag) -> Result<u32>`
    - `prune() -> Result<u64>`
    - `check(read_fraction) -> Result<()>`
    - `size() -> Result<u64>`
- [ ] Tests (folder target in a tempdir):
  - `empty_passphrase_round_trip`
  - `encrypted_round_trip`
  - `wrong_passphrase_rejected`
  - `dedup_stores_identical_file_once` (two copies of the same model: repository growth is below 1.1× one copy)
  - `zstd_compression_on` (an STL fixture shrinks)
  - `cancel_mid_backup_leaves_repo_valid`
- [ ] Measure the release binary size and clean build time before and after, and note them in the PR.
- [ ] Commit `feat(core): rustic repository wrapper for backups`.

### Task 2: Backup config, secrets and events (#93)

**Files:** `migrations/0007_backup.sql`, `src/backup/config.rs`, `src/backup/secrets.rs`, `src/events.rs`, `src/engine.rs`

**Interfaces:**
- Produces:
  - Tables `backup_targets` (id, kind, json settings without secrets, encrypted flag, created_at), `backup_libraries` (library_id, models_on), `backup_runs` (id, kind data|models, library_id, started_at, finished_at, result, bytes_added, error) and `install_predecessors`.
  - `Engine::configure_backup(BackupConfig)` and `Engine::backup_config()`.
  - `Engine::test_target(&Target, &TargetSecrets) -> Result<()>` (reach + write + delete a probe).
  - `secrets::Store { get, set, delete }` over keyring, with an in-memory fallback.
  - Events: `BackupStarted { kind }`, `BackupProgress { kind, done, total }`, `BackupFinished { kind, summary }`, `BackupFailed { kind, error }`, `BackupNeedsAttention(Attention)`.
- [ ] Tests:
  - `config_round_trips_without_secrets` (grep the DB file and settings for the secret: absent)
  - `test_target_detects_read_only_folder`
  - `wrong_passphrase_needs_attention`
  - `missing_keyring_falls_back_to_session_secret`
- [ ] Commit `feat(core): backup config, secret store and events`.

### Task 3: Data snapshot (#94)

**Files:** `src/backup/data.rs`, `src/backup/mod.rs`, `tests/backup_data.rs`

**Interfaces:**
- Produces `Engine::backup_now(BackupScope::{Data, Models(LibraryId), All}) -> Result<RunSummary>`, using decision 2's layout. `VACUUM INTO` runs on the writer connection, so the live DB keeps serving.
- [ ] Tests:
  - `data_snapshot_has_db_photos_settings_not_caches`
  - `db_copy_is_consistent_during_writes` (concurrent tag writes while backing up; the restored DB passes `PRAGMA integrity_check`)
  - `interrupted_backup_resumes` (cancel after the first pack; the second run uploads only the rest)
  - `no_secret_in_snapshot`
- [ ] Commit `feat(core): Printdex data snapshots`.

### Task 4: Model snapshots per library (#95)

**Files:** `src/backup/models.rs`, `src/walk.rs` (expose rule globs), `tests/backup_models.rs`

**Interfaces:**
- Consumes `walk::Rules`.
- Produces `walk::Rules::exclude_globs() -> Vec<String>`, `Engine::estimate_models(library) -> Result<u64>` (sum of known part and asset sizes), and model snapshots tagged `printdex-models:<library uuid>`.
- [ ] Tests:
  - `backup_file_set_equals_scanner_file_set`
  - `offline_library_is_skipped`
  - `estimate_matches_library_size`
  - `defaults_on_for_local_off_for_network`
- [ ] Commit `feat(core): model file snapshots per library`.

### Task 5: Scheduler, retention, checks and attention (#96)

**Files:** `src/backup/scheduler.rs`, `src/engine.rs` (`start_backup_scheduler`), `tests/backup_scheduler.rs`

**Interfaces:**
- Produces:
  - `Engine::start_backup_scheduler() -> BackupScheduler` (handle; drop stops it, like `Monitor`).
  - `scheduler::due(now, state) -> Vec<Job>` as a pure function for tests.
  - `Attention::{DataStale, ModelsStale(LibraryId), ConnectLibrary(LibraryId), TargetUnreachable, TargetFull, Credentials, PassphraseNeeded, CheckFailed}`.
- Timings: data runs 5 min after the last change and at least every 24 h. Models run daily, weekly or manually (setting), and also after a scan with changes once the schedule is due. Forget runs after each run with 7 daily, 4 weekly and 12 monthly snapshots per host + tag. Prune runs at most weekly, and the check monthly with a 2 % read.
- Attention fires at 3 days without a data backup, or 14 days without a model backup.
- Transient errors retry at 1, 5 and 30 min, then hourly.
- [ ] Tests:
  - `data_backup_debounced_5_min`
  - `due_on_launch_runs_soon`
  - `retention_is_per_host_and_tag`
  - `offline_library_skipped_not_failed`
  - `attention_after_thresholds_only`
  - `scheduler_yields_while_scanning`
- [ ] Commit `feat(core): backup scheduler, retention and health`.

## Part B — Backup in the app

### Task 6: Settings › Backup, status line, prompts (#97)

**Files:** `src/backup_ui.rs` (app), `src/view/backup.rs`, `src/view/settings.rs`, `src/view/sidebar.rs`, `locales/en/main.ftl`, `app.rs`

**Interfaces:**
- Consumes Part A's commands and events.
- Produces:
  - `SettingsSection::Backup`.
  - A setup form for each target kind (SFTP hidden on Windows), with Test connection, an encryption toggle (default on for remote and off for folders), passphrase ×2, an acknowledgment checkbox and "Remember on this computer".
  - Per-library model switches with size estimates, and a confirm step before the first model upload.
  - Schedules, Back up now, the repository size and a history list (last 50 runs).
  - A sidebar footer status line (idle / backing up 42 % / last backup 14:02 / needs attention), with a passphrase or credentials prompt.
- [ ] Tests (app state):
  - `setup_requires_ack_when_encrypted`
  - `first_model_backup_needs_confirm`
  - `status_line_states`
  - `sftp_hidden_on_windows` (cfg)
- [ ] Screenshots: setup form (folder and S3), confirm step, status states, history — light and dark.
- [ ] Commit `feat(app): backup settings, status and prompts`.

## Part C — Restore

### Task 7: Restore engine (#98)

**Files:** `src/backup/restore.rs`, `src/engine.rs` (swap on open), `src/db/organize.rs` (predecessor), `src/sidecar.rs`, `tests/backup_restore.rs`

**Interfaces:**
- Produces:
  - `Engine::restore_data(target, secrets, snapshot) -> Result<()>` (stages and writes the marker).
  - `Engine::open` performs the swap (decision 6).
  - `Engine::relink_library(id, new_root)`.
  - `Engine::restore_library_files(id, snapshot, dest)`.
  - `Engine::model_snapshots(model) -> Result<Vec<SnapshotInfo>>` and `Engine::restore_model(model, snapshot, ModelRestoreDest::{Original, Folder(PathBuf)})`.
  - `Db::predecessors()`.
- [ ] Tests:
  - `full_restore_round_trip` (tags, recipe, attempts, photos, numbers equal)
  - `restored_install_gets_new_id_with_predecessor`
  - `predecessor_sidecars_are_own`
  - `restore_beats_newer_sidecars`
  - `relink_rescans_and_keeps_identity`
  - `model_restore_trashes_changed_files_only`
  - `deleted_model_restores_from_backup`
- [ ] Commit `feat(core): restore data, libraries and single models`.

### Task 8: Restore flows in the app (#99)

**Files:** `src/view/restore.rs`, `src/view/empty.rs` (first launch), `src/view/model.rs` (Restore from backup…), `src/view/model.rs` missing banner, `app.rs`, `main.ftl`

- [ ] First launch: "Start fresh" / "Restore from backup". Then target and passphrase, then the point in time (newest first, with host shown and a warning if the snapshot came from another computer), then restart. After the restart, each library asks "It's already here" / "Restore files from backup" / "Skip for now".
- [ ] Model view: "Restore from backup…" lists that model's snapshots, shows which files differ, and restores to the original or another folder. The Missing banner gets the same action.
- [ ] Tests:
  - `first_launch_offers_restore`
  - `library_relink_choices`
  - `model_restore_preview_lists_changed_files`
- [ ] Screenshots of each step.
- [ ] Commit `feat(app): restore flows`.

---

## Self-review notes

- **Spec coverage:**
  - §5 contents: Tasks 3–4.
  - §6 targets and secrets: Tasks 1–2.
  - §7 encryption: Tasks 1, 2, 6.
  - §8 space: Tasks 1, 5, 6.
  - §9 timing: Task 5.
  - §10 restore: Tasks 7–8.
  - §11 health and history: Tasks 5–6.
  - §12 architecture: Tasks 1–5.
  - §13 checks: Task 1.
- **Not covered:** the bandwidth limiter (decision 8, follow-up issue), and changing the passphrase later (restic key management; spec §7). Passphrase change belongs in Task 6's settings as `Engine::change_passphrase(old, new)`, with the test `passphrase_change_keeps_snapshots` added to Task 2.
