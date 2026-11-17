# Printdex NAS Hardening, File Actions, Performance & Releases (Milestone 4) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Large NAS libraries stay correct and fast when files move, mounts drop and shares are slow (part A). The user can rename, move and delete models safely, with confirmation and undo, and open a model in their slicer (part B). Printdex meets the §7 targets on the 100k bench and ships as versioned GitHub releases with an auto-updater (part C).

**Architecture:**
- **Part A, engine only:**
  - `printdex-core` gains an identity pass at the end of each scan (`identity.rs`) that joins "went missing this round" to "new this round" by fingerprint, so moved files keep their model id and user data.
  - A purge pass forgets missing rows after a grace period.
  - A library monitor (`monitor.rs`) owns change detection per library: `notify` watching on local disks, a timer for network shares, debounced targeted rescans, and online/offline transitions.
  - All filesystem access in walk, fingerprint and monitor goes through a small `Fs` trait, so tests can inject latency, failures and vanishing mounts.
- **Part B:**
  - A file-actions module (`file_ops.rs`) performs rename, move and trash.
  - Each action is recorded in a `file_ops` journal before and after, so undo and crash recovery work.
  - The app gets multi-select, confirm dialogs, an undo toast and an "Open with" setting.
- **Part C:**
  - A bench run against the latency wrapper sets the idle numbers.
  - The release workflow builds, packages, signs and publishes on a `v*` tag.
  - Velopack provides the in-app updater.

**Tech Stack:**
- Existing crates: tokio, rayon, sqlx/SQLite, ignore.
- New, all MIT or MIT/Apache:
  - `notify` and `notify-debouncer-full` for watching.
  - `sysinfo` to detect network and removable volumes.
  - `trash` to move files to the OS trash.
  - `velopack` for the updater.
  - `minisign-verify` to check release signatures.
  - `git-cliff` (CI only) for the changelog.

**Specs:**
- v1 design §7 (scanning, change detection, targets) and §8 (file rules).
- Architecture §5.2–5.3 (events, cancellation), §8 (distribution and updates), §11–§12 (milestone order, NAS testing with a latency wrapper, bench in CI).
- Epic #20. Follow-ups #25 (scanner hardening) and #49 (release pipeline).
- DESIGN.md "Model pane", "Toasts" (the saved toast from milestone 3), "Dialogs".

## Decisions made while planning (Claude's calls, flagged for review)

1. **Move and rename detection.** At the end of every scan, parts that went missing this round are matched to parts first seen this round by fingerprint (size + head/tail hash).
   - A **folder model** that moved keeps its id when the set of its part fingerprints matches a new folder's set exactly, or when at least 80% of the old parts match and no other candidate matches better.
   - A **single-file model** keeps its id when one new loose file has its fingerprint.
   - An ambiguous match (two candidates) is not merged. Both stay as they are, and the log says why.
   - When identity moves, tags, collections, notes, recipe, attempts, catalogue number and thumbnails follow (they hang off the model and part ids). The new model row created by the walk this round is folded into the old one.
2. **Missing grace period.** A missing row is kept for **30 days**, then purged on the next scan of that library.
   - Models with user data (tags, notes, a recipe or attempts) are **never** purged automatically. They show in a new **Missing** smart filter with **Forget** (confirmed) and **Show where it was**.
   - An offline library never ages its rows: the 30 days count only scans that actually reached the folder.
3. **Local or network.** `sysinfo` disk info decides per library at add time and on each start:
   - Network: `cifs`/`smb*`/`nfs*`/`afpfs`/`fuse.sshfs`/`davfs`, or a Windows network drive.
   - Removable: removable flag.
   - Local: everything else.
   - Each library gets a **Change detection** setting: Automatic (default), Watch (notify), Rescan every N minutes (5, 15, 60), Manual only.
   - Automatic means: local → Watch; network/removable → every 15 min.
   - Settings › Libraries shows the detected kind ("Network share, checks every 15 minutes").
4. **Watching.**
   - `notify` events are debounced for 2 s, collected into the set of affected model folders, and rescanned as a **subtree scan** (walk only those folders, same diff and apply code).
   - Over 500 affected folders, or a watcher error, falls back to a full library rescan.
   - Watch failure (inotify limit, unsupported FS) switches that library to a 15 min timer and logs it once.
5. **Offline transitions.**
   - The monitor probes each library root every 30 s (one `metadata` call, 3 s timeout) and emits `LibraryOnline/Offline`. Coming back online triggers an incremental rescan.
   - A scan whose root vanishes mid-walk is cancelled and marks the library offline, **without** marking anything missing (fixes #25 "partial list re-roots a model").
6. **Fs trait.** `trait Fs: Send + Sync` with `read_dir`, `metadata`, `open_read` (returns `impl Read + Seek`), `exists`. `RealFs` is the default, and `ChaosFs` in tests adds latency per call, fails a chosen fraction, can "unmount" a subtree, and drops watcher events. The `ignore` walker cannot use a custom fs, so the walk runs over `RealFs` in production and over a custom recursive walker in `ChaosFs` tests that shares the filter rules. *(The filter rules move to `walk::Rules` so both walkers use them.)*
7. **File actions.**
   - **Rename** changes a model's folder name (folder model) or file name (single file), inside its folder. Names are checked for OS-invalid characters and collisions.
   - **Move** puts one or more models into another folder of the same library, or into another library: a rename where possible, otherwise copy + verify size + trash the source.
   - **Delete** moves to the OS trash with the `trash` crate.
   - Every action is confirmed in a dialog that lists what changes ("Move 3 models to Storage/Bins?"). Undo is offered for 10 s in a toast and stays in Edit › Undo (Ctrl+Z) until the next file action, so only the last action can be undone.
   - Undo of a delete restores from the trash on Linux and Windows. macOS cannot restore programmatically, so undo there says "Restore it from the Trash in Finder" and opens the Trash.
   - The DB row and sidecar move with the files. The sidecar always travels inside a folder model. A single-file model's `.<file>.printdex.toml` is moved and renamed alongside.
   - Multi-select in the grid and list: Ctrl/Cmd-click toggles, Shift-click extends, Ctrl+A selects all visible. Enter opens only with one selected.
8. **Journal.**
   - A `file_ops` table records `{id, kind, library, from, to, state: planned|done|undone|failed, at}`.
   - Each step is written before and after its filesystem call.
   - On start, any `planned` op is checked against the disk (did the rename happen?) and the database is fixed to match. The user is told once ("Printdex finished moving Wall hook after a crash").
9. **Open with.**
   - Settings › Apps lets the user pick an app per format (STL, 3MF, OBJ), e.g. PrusaSlicer.
   - The model header's Open button becomes **Open in PrusaSlicer** when set. Its menu also offers "Open with default app" and "Choose app…".
   - The app is launched with `std::process::Command` with the part paths as arguments, never through a shell.
10. **Performance targets** (§7, measured with `printdex bench` on the 100k library behind `ChaosFs` with 2 ms per call):
    - start to usable under 1 s
    - search p95 under 50 ms
    - unchanged rescan reads no file content
    - subtree rescan of one folder under 200 ms
    - idle CPU under 1 % with 3 watched libraries
    - idle RSS under 250 MB with 100k models
    - The bench writes these to a JSON report that CI compares against `bench/baseline.json` (±20 %).
11. **Releases.**
    - Versions use SemVer starting at `0.1.0`. The tag `vX.Y.Z` on `main` triggers `release.yml`.
    - The changelog is generated from conventional commits by git-cliff into `CHANGELOG.md` and the release notes.
    - Build outputs:
      - Linux: AppImage (x86_64).
      - Windows: Setup.exe and portable zip (x86_64).
      - macOS: universal `.app` in a `.dmg`, ad-hoc signed.
    - Velopack (`vpk pack`) produces the update packages and feed (`releases.<channel>.json`) on the GitHub release.
    - Every artifact also gets a minisign `.minisig`. The app verifies the downloaded package's signature against the public key compiled in, before Velopack applies it.
12. **Update consent.** On first launch a one-line banner asks "Check for updates automatically?" (Yes / Not now). Settings › About shows the version, the choice, and **Check now**. A found update downloads in the background and installs on restart, after a "Restart to update" prompt.
13. **Not in this milestone:** Flatpak, ARM Linux/Windows builds, OS code signing, a Samba share in CI (the wrapper first), moving models between machines, batch rename patterns.

## Global Constraints

- No `unsafe` in Printdex code (workspace lint). Dependencies may use it.
- Never modify model file contents. File actions only rename, move or trash whole models, and only on explicit, confirmed user action (v1 §8.3).
- Never lose user data to a flaky share: an offline or half-listed library never marks rows missing or purges.
- The update check is the only background network call, and only after consent (architecture §8).
- Fluent for every new string; tokens only from `theme`.
- CI: PRs run Linux only; bench and Windows on `main`; macOS weekly or with the `ci: full` label; the release workflow runs all three.
- Dependencies stay GPL-3.0-compatible (`cargo-deny`).

## Review Focus

1. **A share drops mid-scan.** Nothing is marked missing, the library shows Offline, and data is intact when it returns. Test: Task 3 `vanishing_mount_mid_walk_marks_nothing_missing`.
2. **A folder renamed on the NAS while the app is closed.** On the next scan the model keeps its tags, recipe, attempts and number. Test: Task 2 `renamed_folder_keeps_identity`.
3. **Two identical copies of a model** (a duplicate folder) are never merged into one, and moving one of them does not steal the other's data. Test: Task 2 `duplicate_copies_are_not_merged`.
4. **Crash in the middle of a move.** The next start reconciles the DB with the disk. Test: Task 6 `planned_op_reconciled_on_start`.
5. **Moving into a folder that already has that name.** Refused with a clear message and nothing touched. Test: Task 6 `move_collision_refused`.

---

## File Structure

```
crates/printdex-core/migrations/0005_monitor_file_ops.sql   libraries.change_mode/interval/kind, file_ops, missing purge index
crates/printdex-core/src/fs.rs                               Fs trait, RealFs, ChaosFs (cfg(test) + feature "chaos")
crates/printdex-core/src/walk.rs                             walk::Rules shared by both walkers; subtree walk
crates/printdex-core/src/identity.rs                         move/rename matching after a scan
crates/printdex-core/src/purge.rs                            grace-period purge, Missing filter queries
crates/printdex-core/src/monitor.rs                          per-library watcher/timer/probe, debounced subtree rescans
crates/printdex-core/src/volume.rs                           local/network/removable detection (sysinfo)
crates/printdex-core/src/file_ops.rs                         rename, move, trash, journal, undo, reconcile
crates/printdex-core/src/scan.rs, engine.rs                  subtree scans, identity + purge passes, new commands/events
crates/printdex-app/src/selection.rs                         multi-select state
crates/printdex-app/src/view/dialogs.rs                      confirm dialogs (move, rename, delete, forget)
crates/printdex-app/src/view/settings.rs                     change detection per library, Apps, About/updates
crates/printdex-app/src/updates.rs                           consent, check, download, verify, restart prompt
crates/printdex-cli/src/bench.rs                             chaos bench, JSON report, baseline compare
.github/workflows/release.yml, bench.yml                     release on tag; bench compare on main
cliff.toml, CHANGELOG.md, packaging/                         changelog config, desktop file, icons, Info.plist
```

---

## Part A — NAS hardening (engine)

### Task 1: Fs trait, chaos wrapper and #25 scanner fixes

**Files:** `src/fs.rs`, `src/walk.rs`, `src/fingerprint.rs`, `src/scan.rs`, `tests/chaos.rs`

**Interfaces:**
- Produces:
  - `pub trait Fs: Send + Sync + 'static`
  - `pub struct RealFs`
  - `pub struct ChaosFs { latency: Duration, fail_rate: f32, unmounted: Vec<PathBuf> }` with `unmount(&self, p)` and `remount(&self, p)`
  - `walk::Rules::keep(&self, rel: &Path, is_dir: bool) -> bool`
  - `Engine::open_with_fs(data, cache, Arc<dyn Fs>)`

- [ ] Tests:
  - `junk_folders_match_case_insensitively` (`$recycle.bin`, `@Recycle`, `@Recently-Snapshot` skipped)
  - `fingerprint_size_mismatch_is_failure` (file grows between walk and hash → `FingerprintFailed`, retried next scan)
  - `symlink_to_offline_target_reports_offline`
  - `dropping_rescan_future_stops_walk` (walk checks a token derived from a drop guard)
  - `scan_lock_file_blocks_second_process` (lock file `data/scan-<library>.lock`, stale after the PID is gone)
  - `symbol_only_search_tokens_are_dropped` (`dragon *` == `dragon -` == `dragon`)
- [ ] Implement `Fs`, move the filter rules to `walk::Rules`, apply the #25 fixes.
- [ ] Commit `feat(core): fs abstraction, chaos wrapper, scanner hardening (#25)`.

### Task 2: Identity across moves and renames

**Files:** `src/identity.rs`, `src/db/files.rs`, `src/scan.rs`, `tests/identity.rs`

**Interfaces:**
- Consumes: the scan's apply result (`Applied { new_models, new_parts, missing_models, missing_parts }`; apply must return it).
- Produces: `identity::reconcile(db, library, &Applied) -> Result<Vec<Moved>>`, where `Moved { model: ModelId, from: String, to: String }`. A new event `EngineEvent::ModelsMoved(Vec<Moved>)`.

- [ ] Tests:
  - `renamed_folder_keeps_identity` (tags, recipe, attempts, number, thumbnails follow)
  - `moved_single_file_keeps_identity`
  - `folder_with_one_part_edited_still_matches` (80% rule)
  - `duplicate_copies_are_not_merged`
  - `moved_folder_sidecar_follows`
- [ ] Implement matching per decision 1 in one transaction. Fold the walker's new model row into the old id: update the old row's path, move the parts, delete the new row.
- [ ] Commit `feat(core): keep model identity across moves and renames`.

### Task 3: Missing grace period, offline safety and the Missing filter

**Files:** `migrations/0005_monitor_file_ops.sql` (purge part), `src/purge.rs`, `src/db/search.rs`, `src/scan.rs`

**Interfaces:**
- Produces:
  - `Smart::Missing`
  - `Engine::forget_model(id)`
  - `purge::run(db, library, now) -> Result<u64>`
  - the constant `MISSING_GRACE_MS = 30 days`
  - `libraries.reached_at` (last scan that reached the root)

- [ ] Tests:
  - `missing_rows_purged_after_grace`
  - `missing_with_user_data_never_purged`
  - `offline_library_does_not_age`
  - `vanishing_mount_mid_walk_marks_nothing_missing` (ChaosFs unmount during walk)
  - `forget_removes_model_and_sidecar_data_row`
- [ ] Implement. Missing models are hidden from normal scopes and listed under Missing.
- [ ] Commit `feat(core): missing grace period and Missing filter`.

### Task 4: Volume detection and library monitor

**Files:** `src/volume.rs`, `src/monitor.rs`, `src/engine.rs`, `src/scan.rs`, migration (change mode columns)

**Interfaces:**
- Produces:
  - `enum VolumeKind { Local, Network, Removable }`
  - `enum ChangeMode { Auto, Watch, Every(u16 /*min*/), Manual }`
  - `Engine::set_change_mode(lib, ChangeMode)` and `Library { kind: VolumeKind, change_mode: ChangeMode }`
  - `Engine::rescan_paths(lib, Vec<String>)` for subtree scans
  - events `LibraryOnline(LibraryId)` and `LibraryOffline(LibraryId)`
  - the monitor starts with the engine and stops on drop

- [ ] Tests (ChaosFs and tokio time pausing):
  - `watch_event_triggers_subtree_rescan_after_debounce`
  - `many_events_fall_back_to_full_rescan`
  - `network_library_rescans_on_timer`
  - `manual_mode_never_rescans`
  - `offline_then_online_triggers_rescan`
  - `watch_failure_falls_back_to_timer`
  - `classify_mount_types` (pure function over fs-type strings)
- [ ] Implement per decisions 3–5.
- [ ] Commit `feat(core): library monitor with watching, timers and offline detection`.

## Part B — File actions, Missing UI, Open with

### Task 5: App: offline/refresh, change detection setting, Missing smart filter

**Files:** `view/sidebar.rs`, `view/settings.rs`, `view/model.rs`, `app.rs`, `locales/en/main.ftl`

- [ ] Tests:
  - `offline_event_dims_library_and_keeps_models`
  - `change_mode_saved_per_library`
  - `missing_filter_lists_missing_models`
  - `forget_requires_confirmation`
- [ ] Sidebar shows offline libraries dimmed with "Offline". Settings › Libraries gains Change detection with the detected kind. The Missing filter appears only when its count > 0. A missing model's view offers Forget and Show where it was.
- [ ] Commit `feat(app): offline libraries, change detection and Missing filter`.

### Task 6: File actions engine and journal

**Files:** `src/file_ops.rs`, migration (`file_ops`), `src/engine.rs`, `tests/file_ops.rs`

**Interfaces:**
- Produces:
  - `Engine::rename_model(id, new_name) -> Result<OpId>`
  - `Engine::move_models(&[ModelId], target: (LibraryId, String /*folder rel*/)) -> Result<OpId>`
  - `Engine::trash_models(&[ModelId]) -> Result<OpId>`
  - `Engine::undo(OpId) -> Result<()>`
  - `Engine::last_op() -> Option<OpSummary>`
  - `Engine::reconcile_ops()`, called by `open`
  - errors `NameInvalid`, `NameTaken`, `TargetOffline`, `TrashUnavailable`, `UndoUnavailable`

- [ ] Tests:
  - `rename_folder_model_moves_rows_and_sidecar`
  - `rename_single_file_moves_its_sidecar`
  - `move_across_libraries_copies_then_trashes`
  - `move_collision_refused`
  - `trash_then_undo_restores` (Linux)
  - `planned_op_reconciled_on_start`
  - `offline_target_refused`
- [ ] Implement per decisions 7–8. The DB update happens after a successful filesystem step, in the same journal entry.
- [ ] Commit `feat(core): rename, move and trash with undo journal`.

### Task 7: App: multi-select, file action dialogs, undo, Open with

**Files:** `src/selection.rs`, `view/dialogs.rs`, `view/grid.rs`, `view/list.rs`, `view/model.rs`, `view/settings.rs`, `app.rs`

- [ ] Tests:
  - `ctrl_click_toggles_selection`
  - `shift_click_extends_selection`
  - `delete_key_asks_before_trashing`
  - `undo_toast_undoes_last_action`
  - `ctrl_z_undoes_last_file_action`
  - `rename_dialog_shows_name_taken_error`
  - `open_with_uses_chosen_app`
- [ ] Rename (F2), Move to… (folder picker limited to library roots), Move to trash (Delete) from the model header menu, the card context menu and keys. Confirm dialogs per DESIGN.md "Dialogs". Undo toast for 10 s. Settings › Apps for Open with.
- [ ] Screenshots for the PR (both modes): multi-select, move dialog, undo toast, offline library, Missing filter.
- [ ] Commit `feat(app): file actions with undo, multi-select and Open with`.

## Part C — Performance and releases

### Task 8: Performance pass

**Files:** `crates/printdex-cli/src/bench.rs`, `bench/baseline.json`, `.github/workflows/bench.yml` (or the existing main-only job)

- [ ] `printdex bench --chaos-latency 2ms --report out.json` measures:
  - start-to-usable
  - search p50/p95
  - unchanged rescan (with bytes read)
  - subtree rescan
  - idle CPU over 60 s with 3 watched libraries
  - RSS
- [ ] Fix whatever misses decision 10's numbers. Write the measured numbers into DESIGN/architecture as the set targets.
- [ ] CI on `main` compares against the baseline and fails on a >20 % regression.
- [ ] Commit `perf: chaos bench, baseline and fixes`.

### Task 9: Release pipeline, versioning and changelog (#49)

**Files:** `.github/workflows/release.yml`, `cliff.toml`, `CHANGELOG.md`, `packaging/` (desktop file, icons, Info.plist), `Cargo.toml` versions

- [ ] On tag `v*`, a matrix (ubuntu, windows, macos) builds `--release`, then `vpk pack` per OS, creates the `.dmg` on macOS, ad-hoc `codesign -s -`, and signs every artifact with minisign (secret `MINISIGN_KEY`).
- [ ] The workflow uploads to a draft GitHub release with git-cliff notes. Max publishes it.
- [ ] Dry run on a `v0.1.0-rc.1` tag from the branch. Artifacts install and launch on Linux in the cloud.
- [ ] **Needs from Max:** add the `MINISIGN_KEY` and `MINISIGN_PASSWORD` secrets (the plan includes a script that generates the pair locally and prints the public key to commit).
- [ ] Commit `build: release pipeline, versioning and changelog (#49)`.

### Task 10: Auto-updater with consent

**Files:** `src/updates.rs` (app), `view/settings.rs` (About), `app.rs`, `Cargo.toml` (velopack, minisign-verify)

- [ ] Tests:
  - `no_check_without_consent`
  - `consent_banner_once`
  - `bad_signature_rejected` (fixture package + wrong key)
  - `update_installs_only_after_restart_prompt`
- [ ] `velopack::VelopackApp::build().run()` first in `main`. Checks run only with consent (on start + every 24 h + Check now). Downloads are verified with minisign before `apply_updates_and_restart` is offered.
- [ ] Commit `feat(app): auto-updater with consent and signature check`.

---

## Self-review notes

- Spec coverage:
  - v1 §7 change detection: Tasks 4–5.
  - Move detection: Task 2.
  - Grace period: Task 3.
  - §8 file rules: Tasks 6–7.
  - Open with: Task 7.
  - Targets: Task 8.
  - Architecture §8: Tasks 9–10.
  - §12 NAS testing: Tasks 1, 3, 4.
  - #25 items: Task 1, plus Task 3 (partial-list re-rooting).
  - #49: Task 9.
- Split into PRs: part A (Tasks 1–4), part B (Tasks 5–7), part C (Tasks 8–10).
