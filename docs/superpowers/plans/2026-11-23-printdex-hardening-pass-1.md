# Hardening pass 1: diagnostics and data loss — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Printdex a log file, a crash record with a next-start notice, and "Report a bug". Then close every known path where Printdex can lose or corrupt a user's files or data.

**Architecture:** A new `printdex-core::diagnostics` module owns the log file, the panic hook, crash records, redaction and the diagnostics zip. Both the app and the CLI call it. A new `printdex-core::fsx` module owns every write, rename and copy of user data: a durable write, a rename that never replaces, and a content-verified tree copy. It also has a fault-injection hook that tests use to fail or interfere with the *n*th operation under a given folder. All writers and file actions move onto `fsx`, and the tests drive them through every failure point.

**Tech Stack:** Rust 1.94, tokio, iced 0.14, sqlx/SQLite. New crates: `tracing-appender` 0.2 (MIT) and `rustix` 1.x with the `fs` feature (Apache-2.0/MIT, already in `Cargo.lock`). Already in the tree: `zip` 9, `xxhash-rust` (xxh3), `url` 2.5.9, `rfd`, `opener`.

**Spec:** `docs/superpowers/specs/2026-11-23-printdex-hardening-design.md`, which covers pass 1 and pass 2's "Report a bug", log file and panic hook. Later passes get their own plans when they start, because each pass's sweep findings feed the next plan.

## Global Constraints

- No `unsafe` in Printdex crates (`unsafe_code = "forbid"`). Platform calls go only through safe crate APIs.
- New dependencies must be GPL-3.0-compatible, and `cargo deny check` must pass.
- Feature PRs target `dev`. Part A and part B are separate PRs. If part A is still in review when part B starts, part B's branch stacks on it.
- No migration or compatibility work for older installs (no one uses Printdex yet).
- UI text goes through `crates/printdex-app/locales/en/main.ftl` and `i18n::t`, in the project's existing voice: plain words, no exclamation marks.
- Nothing is sent anywhere automatically. In shared text, the home folder and library paths are replaced with placeholders.
- Bench budgets in `bench/baseline.json` must still pass (`printdex bench check`).
- Every fix starts with a test that fails without it.
- Before each commit: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test -p <crate> <filter>` for the changed area. Set `DATABASE_URL=sqlite://$PWD/target/dev.db` in the same command if queries change.
- Each newly found edge case is added as a checklist item to the part's GitHub issue under epic #143.

## Review Focus

1. **The data folder is read-only or full at start.** The app must still open (logging to stderr only) and must not crash in the log or panic setup. Test in Task 1.
2. **A panic on a tokio worker or rayon thread, or a panic inside the hook itself.** A crash file must still be written, and the hook must never panic (no `unwrap` in it). Test in Task 1.
3. **Paths that identify the user in shared text: Windows `C:\Users\max\…`, either slash direction, any letter case, and library roots.** All are redacted. Test in Task 1.
4. **The disk fills during a durable write.** The original file stays intact and no temporary file is left. Test in Task 5.
5. **The target drive fills partway through a move between drives.** The partial copy is removed, the source is untouched, and the journal says the action failed. Test in Task 7.

---

## Part A: Diagnostics (PR "hardening: log file, crash notice and Report a bug")

### Task 1: `diagnostics` core module

**Files:**
- Create: `crates/printdex-core/src/diagnostics.rs`
- Modify: `crates/printdex-core/src/lib.rs` (add `pub mod diagnostics;`), `Cargo.toml` and `crates/printdex-core/Cargo.toml` (add `tracing-appender = "0.2"` and `tracing-subscriber`; move `zip` from dev-dependencies to dependencies)
- Test: unit tests in `diagnostics.rs`

**Interfaces:**
- Produces:
  - `pub fn log_dir(data_dir: &Path) -> PathBuf` returns `data_dir/logs`.
  - `pub fn init(log_dir: &Path, filter: &str) -> Option<tracing_appender::non_blocking::WorkerGuard>` installs the global subscriber: stderr plus a daily rolling file `printdex.log` with `max_log_files(7)`. If the folder can't be created or written, it installs stderr only and returns `None`.
  - `pub fn install_panic_hook(log_dir: PathBuf, version: &'static str)` writes `crash-<unix_ms>.txt` and then calls the previous hook.
  - `pub struct CrashReport { pub path: PathBuf, pub at_ms: i64, pub message: String, pub location: String }`
  - `pub fn unseen_crash(log_dir: &Path) -> Option<CrashReport>` returns the newest crash file newer than the `logs/.crash-seen` marker's mtime.
  - `pub fn mark_crashes_seen(log_dir: &Path) -> io::Result<()>`
  - `pub struct Redactor` with `pub fn new(home: Option<&Path>, libraries: &[PathBuf]) -> Redactor` and `pub fn apply(&self, text: &str) -> String`. Library *i* becomes `<library i+1>` and the home folder becomes `~`. Matching ignores case and treats `\` and `/` as the same.
  - `pub fn system_summary(version: &str, renderer: &str) -> String` lists the version, `std::env::consts::{OS, ARCH}`, the OS version from `sysinfo::System::long_os_version()` and the renderer.
  - `pub fn save_bundle(log_dir: &Path, dest: &Path, summary: &str, redactor: &Redactor) -> io::Result<()>` writes a zip holding `summary.txt`, the 3 newest `printdex.log*` files and the 5 newest `crash-*.txt`, each passed through `redactor`.

The crash file format is fixed, because `unseen_crash` parses it:

```
Printdex <version> crashed
time: <unix_ms>
os: <OS> <ARCH>
thread: <name or "unnamed">
message: <payload &str/String, else "non-string panic">
location: <file>:<line>:<col>
backtrace:
<std::backtrace::Backtrace::force_capture()>
```

- [ ] **Step 1: Write failing tests**
  - `init_without_writable_folder_falls_back_to_stderr`: pass a path whose parent is a file. Expect `None` and no panic. Run it in a subprocess-free way by testing `make_file_writer(dir) -> io::Result<_>` returning `Err`, with `init` built on it.
  - `panic_hook_writes_crash_file_from_any_thread`: install the hook inside `std::panic::catch_unwind`-isolated `std::thread::spawn(|| panic!("boom"))`, then `unseen_crash(dir)` returns `message == "boom"` and a `location` ending in `diagnostics.rs:<n>:<n>`. Use a test-only `fn write_crash(dir, info_parts)` seam so the global hook is installed once per test binary.
  - `crash_seen_marker_hides_older_crashes`: after `mark_crashes_seen`, `unseen_crash` returns `None`, and a newer file shows again.
  - `redacts_home_and_libraries_any_slash_any_case`: `Redactor::new(Some("C:\\Users\\Max"), &["D:\\Prints".into()])` turns `"c:/users/max/x and D:\\prints\\a.stl"` into `"~/x and <library 1>\\a.stl"`, and `/home/max/.local` with home `/home/max` turns into `~/.local`.
  - `bundle_holds_summary_logs_and_crashes_redacted`: build a folder with 4 log files and 6 crash files containing the home path. Open the zip and assert the 1 + 3 + 5 entries and that no entry contains the home path.
- [ ] **Step 2: Run `cargo test -p printdex-core diagnostics`.** Expected: fails to compile (module missing).
- [ ] **Step 3: Implement the interfaces above.** The hook body uses only fallible calls with `let _ =` and never `unwrap`, because it must never panic.
- [ ] **Step 4: Run `cargo test -p printdex-core diagnostics`.** Expected: PASS.
- [ ] **Step 5: Run `cargo deny check`.** Expected: ok. Then commit `feat(core): log file, crash records and diagnostics bundle`.

### Task 2: Wire diagnostics into the app and CLI, with a crash notice

**Files:**
- Modify: `crates/printdex-app/src/main.rs` (resolve `data_dir` before logging, then call `diagnostics::init` and `install_panic_hook`, and keep the guard alive until `run()` returns), `crates/printdex-app/src/app.rs` (`Options.crash: Option<CrashReport>`, plus `Message::CrashNotice(CrashUx)` with `Show`, `Report` and `Dismiss`), `crates/printdex-app/src/view/mod.rs` (a notice pane above the grid, reusing the existing banner style from the relink banner), `crates/printdex-cli/src/main.rs` (the same `init` and hook, with the data folder from its `--data-dir` and default)
- Modify: `crates/printdex-app/locales/en/main.ftl` (`crash-notice = Printdex closed unexpectedly last time.`, `crash-show = Show file`, `crash-report = Report a bug`, `crash-dismiss = Dismiss`)
- Test: `app.rs` tests

**Interfaces:**
- Consumes: Task 1's `init`, `install_panic_hook`, `unseen_crash`, `mark_crashes_seen` and `CrashReport`.
- Produces: `Message::CrashNotice(CrashUx::Report)` opens Settings › About scrolled to "Report a bug" (Task 3).

- [ ] **Step 1: Write failing tests.** `crash_notice_shows_once`: an `App` booted with `Options.crash = Some(..)` has `a.crash.is_some()`. After `update(Message::CrashNotice(CrashUx::Dismiss))` it is `None`, and the marker file exists in the temp log folder. `no_crash_no_notice`: with `crash: None`, nothing renders. Assert that `view::crash_notice(&a)` returns `None`.
- [ ] **Step 2: Run `cargo test -p printdex-app crash_notice`.** Expected: FAIL.
- [ ] **Step 3: Implement.** Main reads `unseen_crash` before installing the hook. `Show` calls `opener::reveal(path)`.
- [ ] **Step 4: Run the tests again.** Expected: PASS. Then run `cargo run -p printdex-app -- --data-dir /tmp/pd --config-dir /tmp/pdc`, kill it with `kill -ABRT`, write a crash file by hand, relaunch, and take a screenshot of the notice (see `cloud-app-screenshots` in memory). Check that `/tmp/pd/logs/printdex.log.*` exists.
- [ ] **Step 5: Commit** `feat(app): log to a file and show a notice after a crash`.

### Task 3: Report a bug (Settings › About)

**Files:**
- Create: `crates/printdex-app/src/report.rs` (pure logic: `issue_url` and the summary inputs)
- Modify: `crates/printdex-app/src/view/settings.rs` (a "Report a bug" block under About with two buttons, built like `updates()`), `crates/printdex-app/src/app.rs` (`Message::Report(ReportUx)` with `OpenIssue`, `SaveDiagnostics`, `Saved(Result<PathBuf, String>)`, using the existing toast to confirm), `crates/printdex-app/Cargo.toml` (`url.workspace = true`, and add `url` to the workspace deps), `main.ftl` (`report-title = Report a bug`, `report-body = Opens a new issue on GitHub, or saves a file with logs you can send another way. Nothing is sent without you.`, `report-open-issue = Open a GitHub issue`, `report-save = Save diagnostics…`, `report-saved = Diagnostics saved`)
- Test: `report.rs` unit tests

**Interfaces:**
- Consumes: Task 1's `Redactor`, `system_summary`, `save_bundle` and `CrashReport`. The renderer comes from the viewer's existing software-renderer probe (`viewer.rs:60`): `"CPU (software)"` when the probe has fired, `"GPU"` once a GPU frame is drawn, and `"unknown"` before either.
- Produces: `pub fn issue_url(summary: &str, crash: Option<&CrashReport>, redactor: &Redactor) -> String`

URL: `https://github.com/wolfedme/printdex/issues/new?labels=type%3A+bug&title=<…>&body=<…>`, encoded with `url::form_urlencoded`. The title is `Crash: <message, first 80 chars>` when there's a crash, else empty. The body has three sections: `### What happened` (empty, for the user to fill in), `### System` (the redacted summary), and `### Last crash` (the redacted message and location only, no backtrace). The whole URL stays ≤ 7,500 bytes, cutting the crash message first.

- [ ] **Step 1: Write failing tests.** `issue_url_prefills_system_and_crash`: decode the query, then assert `labels == "type: bug"`, that the body contains `### System` and the version, and that the title starts with `Crash: boom`. `issue_url_never_contains_home`: a crash message with the home path contains `~` and not the path. `issue_url_stays_under_limit`: a 50 KB message gives a URL of ≤ 7,500 bytes. `issue_url_without_crash_has_no_crash_section`.
- [ ] **Step 2: Run `cargo test -p printdex-app report`.** Expected: FAIL.
- [ ] **Step 3: Implement.** `OpenIssue` calls `opener::open_browser(url)`. `SaveDiagnostics` opens `rfd::AsyncFileDialog::save_file()` with the default name `printdex-diagnostics-<yyyy-mm-dd>.zip`, then runs `save_bundle` on `spawn_blocking`. The redactor gets the library roots from `app.libraries`.
- [ ] **Step 4: Run the tests again.** Expected: PASS. Take a screenshot of Settings › About for the PR.
- [ ] **Step 5: Commit** `feat(app): Report a bug opens a prefilled issue or saves diagnostics`. Then open PR A into `dev` with `Refs #143`, and a sub-issue "Pass 1A: diagnostics" closed by hand when it merges.

---

## Part B: Data loss and corruption (PR "hardening: no silent data loss")

### Task 4: `fsx` module with fault injection

**Files:**
- Create: `crates/printdex-core/src/fsx.rs`, `crates/printdex-core/src/fsx/faults.rs`
- Modify: `crates/printdex-core/Cargo.toml` (`rustix = { version = "1", features = ["fs"] }` under `[target.'cfg(unix)'.dependencies]`, and a `fault-injection` feature that `[dev-dependencies] printdex-core = { path = ".", features = ["fault-injection"] }` turns on for its own integration tests), `lib.rs`

**Interfaces:**
- Produces:
  - `pub enum Durability { UserData, Cache }`. `UserData` fsyncs the temporary file before the rename and, on Unix, the parent folder after it. `Cache` skips both.
  - `pub fn durable_write(path: &Path, bytes: &[u8], d: Durability) -> io::Result<()>` writes a temporary file named `.<name>.tmp-<uuid v7 simple>` in the same folder. It never creates folders. It removes the temporary file on any error.
  - `pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()>` returns `ErrorKind::AlreadyExists` if `to` exists. On Linux and macOS it calls `rustix::fs::renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE)`. On Windows, a file goes through `std::fs::hard_link` and then `remove_file(from)`, and a folder goes through `std::fs::rename` (Windows refuses to replace a folder). Where the filesystem doesn't support the call (`Unsupported`, `EINVAL`, or `PermissionDenied` from `hard_link` on FAT or SMB), it falls back to: `to` missing → `std::fs::rename` → check that `from` is gone and `to` exists.
  - `pub fn copy_tree_verified(from: &Path, to: &Path) -> io::Result<()>` refuses up front with `ErrorKind::Unsupported` and the message `"contains links or special files"` if the tree holds a symlink or special file. It copies, compares every file's xxh3-64 and size, and removes `to` on any failure.
  - `pub fn sweep_temp_files(dir: &Path, older_than: Duration)` removes `.*.tmp-*` and `.printdex-*.tmp` entries, one level deep.
  - `faults::arm(root: &Path, op: Op, nth: usize, action: Action) -> FaultGuard`, with `Op::{Write, Rename, Copy, Remove, Trash}` and `Action::{Fail(io::ErrorKind), Before(Box<dyn Fn(&Path) + Send + Sync>)}`. It matches paths starting with `root`, counts per guard, and disarms on drop. When the feature is off, `faults::check(op, path) -> io::Result<()>` is a no-op `#[inline]`.

- [ ] **Step 1: Write failing tests** (unit tests in `fsx.rs`):
  - `durable_write_replaces_and_leaves_no_temp`
  - `durable_write_full_disk_keeps_original`: arm `Write`, `Fail(StorageFull)`. The original bytes are unchanged and the folder has no temporary file. This is review focus 4.
  - `durable_write_never_creates_folders`
  - `rename_no_replace_refuses_existing_file` and `rename_no_replace_refuses_existing_folder`: the target is untouched and the source still exists.
  - `rename_no_replace_moves_when_free`
  - `copy_tree_refuses_symlinks` (Unix only)
  - `copy_tree_detects_changed_content`: arm `Copy`, `Before(..)` flipping one byte in the source after its copy. Expect an `Err`, with `to` removed.
  - `copy_tree_full_target_cleans_up`: `Copy` nth 2, `Fail(StorageFull)`. `to` is gone and `from` is intact. This is review focus 5.
  - `sweep_removes_old_temp_only`
- [ ] **Step 2: Run `cargo test -p printdex-core fsx`.** Expected: FAIL.
- [ ] **Step 3: Implement.** Every function calls `faults::check` before each operation.
- [ ] **Step 4: Run the tests again.** Expected: PASS. Then run `cargo deny check`.
- [ ] **Step 5: Commit** `feat(core): durable writes, no-replace rename and verified copies`.

### Task 5: Move every writer onto `durable_write`

**Files (modify):**
- `crates/printdex-core/src/sidecar.rs:233` `write_atomic` keeps its size and folder checks and then calls `durable_write(.., UserData)`.
- `crates/printdex-core/src/thumbs.rs:700` `store` uses `Cache`, keeping `create_dir_all` (cache folders may be created).
- `crates/printdex-core/src/sync/store.rs:143` `write_atomic` uses `UserData` (it creates its folder itself first, as today).
- `crates/printdex-core/src/sync/run.rs:1324` writes synced photos with `UserData`.
- `crates/printdex-core/src/photos.rs:51` imports photos by copying to a temporary file and then calling `rename_no_replace` to `dest`.
- `crates/printdex-core/src/backup/restore.rs:300` writes the `READY` marker with `UserData`.
- `crates/printdex-app/src/settings.rs:213` and `crates/printdex-app/src/apps.rs:91` use `UserData`.
- `crates/printdex-core/src/scan.rs` calls `sweep_temp_files(model folder, 1 h)` for each folder holding a sidecar it visits, and `engine.rs` calls it on open for `data_dir`, `photos/*` and the sync folder.

- [ ] **Step 1: Write failing tests.** `settings_save_uses_unique_temp`: two threads saving at once both succeed and leave only `settings.toml`. Today they share `settings.toml.tmp`. `sidecar_write_failure_keeps_old_sidecar`: arm `Rename` `Fail(StorageFull)` under the library folder, then change a tag. The old sidecar text is unchanged. `synced_photo_partial_write_leaves_no_file`. `scan_sweeps_stale_temp_next_to_sidecar`.
- [ ] **Step 2: Run the tests.** Expected: FAIL.
- [ ] **Step 3: Switch each call site.**
- [ ] **Step 4: Run `cargo test -p printdex-core` and `cargo test -p printdex-app settings`.** Expected: PASS.
- [ ] **Step 5: Commit** `fix: every user-data write is durable and atomic`.

### Task 6: Keep broken config files instead of overwriting them

**Files:**
- Modify: `crates/printdex-app/src/settings.rs:200` (`load`) and `crates/printdex-app/src/apps.rs:84`. On a parse error, rename the file to `<name>.broken-<unix_ms>` (with `rename_no_replace`), log a warning, and return defaults. The app shows the existing toast "Settings file was unreadable and was set aside" (`settings-broken` in `main.ftl`).
- Modify: `crates/printdex-core/src/sidecar.rs` (around line 167). Find where a sidecar that fails to parse is later overwritten by the writer. Before the first write over it, set it aside as `.<name>.broken-<unix_ms>`.

- [ ] **Step 1: Write failing tests.** `broken_settings_are_set_aside`: write `"mode = 3 ="`, load, then assert defaults plus one `settings.toml.broken-*` file holding the original text. `broken_sidecar_is_set_aside_before_rewrite` (integration test in `tests/it/sidecars.rs`): corrupt a sidecar, edit the model's tags, let the writer run, then assert the new sidecar parses and the broken one exists with the original bytes.
- [ ] **Step 2: Run the tests.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the tests again.** Expected: PASS.
- [ ] **Step 5: Commit** `fix: set broken settings and sidecars aside instead of overwriting`.

### Task 7: File actions never overwrite and survive a failure at any step

**Files:**
- Modify: `crates/printdex-core/src/file_ops.rs`
  - `:209` batch duplicate check compares `to_lowercase()` of `to_rel` (Unicode, on every OS).
  - `:409` `case_only` uses `to_lowercase()` instead of `eq_ignore_ascii_case`.
  - `move_path` (`:625`) uses `fsx::rename_no_replace`. `AlreadyExists` maps to `CoreError::NameTaken(base)`. The cross-drive branch uses `fsx::copy_tree_verified`, and the `Unsupported` links error maps to a new `CoreError::HasLinks(PathBuf)`.
  - `copy_tree` and `tree_listing` are deleted (replaced by `fsx`).
  - `undo_step` and the sidecar moves use `rename_no_replace`.
  - Trash goes through `faults::check(Op::Trash, ..)`.
- Modify: `crates/printdex-core/src/error.rs` (`HasLinks(PathBuf)` with the message "this folder contains links, which Printdex can't move between drives") and `crates/printdex-app/src/files.rs` (toast text for it, using `files-has-links` in `main.ftl`).
- Test: `crates/printdex-core/tests/it/file_ops.rs`

**Interfaces:**
- Consumes: Task 4's `rename_no_replace`, `copy_tree_verified` and `faults`.

- [ ] **Step 1: Write failing tests:**
  - `move_two_models_differing_only_in_case_is_refused`: `A/Benchy.stl` and `B/benchy.stl` moved into `C` give `NameTaken`, and nothing has moved.
  - `file_appearing_at_target_is_never_overwritten`: arm `Rename` nth 0 with `Before(|to| write "theirs" to the target)`. The action fails, `"theirs"` is intact, the source is intact, and the op state is `Failed`.
  - `fault_at_every_step_loses_nothing`: three models moved across two folders with sidecars. For `n` in `0..` (until a run makes no fault fire) and for each `Op`, arm `Fail(Other)` at nth `n`, run `move_models`, then assert the invariant: every original file's bytes exist exactly once at either the source or the target path, every sidecar exists once, and `db.model_detail` paths match the disk. Then run `reconcile_ops()` and assert the same.
  - `cross_drive_move_with_symlink_is_refused` (Unix): force the cross-device branch with a test seam `fsx::faults` `Rename` `Fail(CrossesDevices)`. Expect `HasLinks`, with the source untouched.
  - `cross_drive_move_full_target_cleans_up`: `Rename` `Fail(CrossesDevices)` and then `Copy` nth 1 `Fail(StorageFull)`. The source is intact and the target is absent. This is review focus 5.
- [ ] **Step 2: Run `cargo test -p printdex-core --test it file_ops`.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the tests again.** Expected: PASS. Then run the whole `cargo test -p printdex-core --test it`, because file actions touch identity and sidecars.
- [ ] **Step 5: Commit** `fix(file actions): never overwrite, verify copies, refuse links across drives`.

### Task 8: Make the restore swap resumable

**Files:**
- Modify: `crates/printdex-core/src/backup/restore.rs:154` (`apply_staged`) and `:96` (`move_any` uses `rename_no_replace`).
- Test: `crates/printdex-core/tests/it/backup_restore.rs`

New algorithm. Every run is idempotent, and one run ends the swap:

1. If `staging/READY` is missing, delete the staging folder (unchanged).
2. Read `staging/SWAP`, which holds the name of the `pre-restore-<ms>` folder. If it's missing, create the name and `durable_write` it first.
3. For each `name` in `[DB_FILE, PHOTOS]`: if `staging/name` exists, move the live `name` aside (for `DB_FILE`, together with `-wal` and `-shm`) into the pre-restore folder *if the live one exists and the aside copy doesn't*. Then move `staging/name` to `data_dir/name`.
4. Only after both names are done, delete `staging`.

A missing `staging/DB_FILE` alongside `READY` and `SWAP` now means "the database step is done", not "unfinished".

- [ ] **Step 1: Write failing tests.** `interrupted_swap_keeps_restored_photos`: stage a restore with a DB and photos. Arm `Rename` `Fail(Other)` at nth `k` for each `k`, run `apply_staged`, and expect `Err`. Then run `apply_staged` again unarmed. Assert that `data_dir/printdex.db` is the restored one, the restored photos are present, the old DB and photos sit in exactly one `pre-restore-*` folder, and `staging` is gone. `swap_twice_is_noop`.
- [ ] **Step 2: Run `cargo test -p printdex-core --test it backup_restore`.** Expected: `interrupted_swap_keeps_restored_photos` fails for the step between the DB and the photos.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run the tests again.** Expected: PASS.
- [ ] **Step 5: Commit** `fix(restore): finish an interrupted swap instead of discarding it`.

### Task 9: Sweep the remaining destructive paths

This task works through the spec's pass 1 **Sweep** list. Each finding gets the same test-first cycle as above and is listed in the pass 1B issue. Check every item and record its outcome (fixed, already safe because of a named test, or follow-up issue):

- [ ] Every non-test `remove_file`, `remove_dir_all`, `trash::`, `rename` and `DELETE FROM` call (`rg -n 'remove_(file|dir_all)|trash::|fs::rename|DELETE FROM' crates/*/src`). Each one is either routed through `fsx` or has a one-line comment saying why it's safe.
- [ ] Undo after an external change: undo a move after the user renamed the target by hand. Expected: a refusal with a reason, and nothing overwritten. Add `undo_after_external_rename_is_refused`.
- [ ] The sidecar writer racing a move: the debounced write lands after the model moved. Expected: no sidecar is recreated at the old path (`write_atomic` never creates folders, so check it for a folder that still exists). Add `sidecar_write_after_move_goes_to_new_path`.
- [ ] Missing grace period: a library offline for 31 days and then back purges nothing it can still find. `purge_missing` must only run after a successful walk of that library. Add `offline_library_is_not_purged`.
- [ ] Sync merge with clock skew: an install whose clock is 1 day ahead doesn't win every later edit forever. Check the hybrid clock's bound in `sync/state.rs`, and add a test if none exists.
- [ ] Backup prune: `forget` never removes the only snapshot holding a model's files. Check with the existing `backup_repo` tests, and add `prune_keeps_last_snapshot_with_models`.
- [ ] Run the whole suite: `cargo test --workspace`. Expected: PASS. Run `cargo run -p printdex-cli --release -- bench run --report` and then `bench check`. Expected: within budgets.
- [ ] Commit each fix separately, then open PR B into `dev` with `Refs #143` and screenshots of any new messages.
