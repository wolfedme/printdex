# Hardening Pass 2 (Crashes) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Printdex never dies silently. A panic in a background task is logged and the task restarts, while panic-prone code in app and core is either handled or carries a recorded proof that it is safe. A broken data folder or odd UI state shows a clear message instead of crashing.

**Architecture:** Panics that are caught and recovered from (background tasks, thumbnail workers) are written as `recovered-*` files, so the next start doesn't claim the app crashed. A supervisor in core restarts engine background tasks after a panic and emits `EngineEvent::TaskStopped` once it gives up. Startup errors are classified into specific `CoreError` variants, and each one gets its own screen. Clippy's panic lints are turned on for non-test code in every crate, and every remaining site is either fixed or carries an `#[expect(..., reason = "...")]`.

**Tech Stack:** Rust 1.94, tokio, sqlx/SQLite, iced 0.14, clippy restriction lints.

**Spec:** `docs/superpowers/specs/2026-11-23-printdex-hardening-design.md` (section "Pass 2 — Crashes and panics"). The log file, panic hook, crash notice and Report a bug already shipped in pass 1 (PR #144).

## Global Constraints

- Never add `unsafe` code (`unsafe_code = "forbid"`).
- No one uses Printdex yet: DB and file formats may change without migrations for old installs.
- Every fix comes with a test that reproduces it. Write the test first and watch it fail.
- One PR for the pass, into `dev`, with the `ci: full` label. It must be green on Linux, Windows and macOS. The Linux PR job must stay under 5 minutes.
- Nothing is uploaded automatically. User-facing text lives in `crates/printdex-app/locales/en/main.ftl`, written in plain sentences.
- Gate checks: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `SQLX_OFFLINE=true cargo test --workspace`.

## Review Focus

1. A panic inside a supervised task that keeps panicking (every restart) must not spin. Restarts are capped, and the UI says the task stopped. (Task 2 test `gives_up_after_three_panics`.)
2. A newer-schema or corrupt database must never be modified by opening it. Its bytes stay identical. (Task 3 tests `newer_schema_is_refused_untouched` and `corrupt_db_is_reported_untouched`.)
3. Setting a corrupt database aside must keep the old file under a new name, never delete it. (Task 3 test `set_aside_keeps_old_file`.)
4. A recovered panic must not trigger the "closed unexpectedly" notice, but it must appear in Save diagnostics. (Task 1 tests.)
5. A model selected in the UI that disappears (deleted, forgotten, library removed) must not panic `view()` or `update()`. (Task 6 test `deleted_selected_model_clears_selection`.)

---

### Task 1: Recovered panics are recorded apart from crashes

**Files:**
- Modify: `crates/printdex-core/src/diagnostics.rs`
- Modify: `crates/printdex-core/src/cpu.rs:12`, `crates/printdex-core/src/thumbs.rs:269` (wrap the existing `catch_unwind` calls)

**Interfaces:**
- Produces:
  - `pub fn recovering<T>(f: impl FnOnce() -> T) -> T`. It sets a thread-local "recovering" depth around `f`; it does not catch.
  - `pub struct Recovering<F>(F)`, a future wrapper that sets the same thread-local around every `poll` of `F`, with `pub fn recovering_future<F: Future>(f: F) -> Recovering<F>`.
  - The panic hook writes `recovered-<ms>[-n].txt` (with the extra line `recovered: yes`) instead of `crash-…` when the depth is above 0. `unseen_crash` keeps reading only `crash-*`. `save_bundle` includes `recovered-*`.

- [ ] **Step 1: Write failing tests in `diagnostics.rs` tests.**
  - `recovered_panic_writes_recovered_file`: install the hook on a temp dir (use the existing test helper that installs and restores the hook), run `catch_unwind(|| recovering(|| panic!("boom")))`, assert one `recovered-*.txt` containing `recovered: yes` and `message: boom`, and no `crash-*`.
  - `recovered_panic_is_not_unseen_crash`: same setup, then assert `unseen_crash(dir)` is `None`.
  - `recovering_future_marks_panics`: in a `#[tokio::test]`, run `tokio::spawn(recovering_future(async { panic!("x") })).await` and assert it is an error and wrote a `recovered-*` file.
  - `bundle_includes_recovered_files`: assert `save_bundle` puts the `recovered-*` file in the zip.
- [ ] **Step 2: Run them.** `cargo test -p printdex-core --lib diagnostics` should fail (the functions don't exist).
- [ ] **Step 3: Implement.** Use a `thread_local! { static RECOVERING: Cell<u32> }` with a guard that decrements on drop, including during unwinding. Wrap the `catch_unwind` closures in `cpu.rs` and `thumbs.rs` in `recovering`.
- [ ] **Step 4: Run** `cargo test -p printdex-core --lib`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(diagnostics): record recovered panics apart from crashes`.

### Task 2: Supervised engine background tasks

**Files:**
- Create: `crates/printdex-core/src/supervise.rs`
- Modify:
  - `crates/printdex-core/src/events.rs` (new event)
  - `sidecar.rs:574` (writer task)
  - `thumbs.rs:314` (workers)
  - `monitor.rs:104,154`
  - `sync/run.rs:205`
  - `sync/api.rs:598`
  - the backup scheduler spawn in `backup/scheduler.rs` (or wherever `start_backup_scheduler` spawns)
  - `lib.rs` (`mod supervise`)
- Modify: `crates/printdex-core/src/fsx/faults.rs` (panic injection behind `fault-injection`)
- Modify:
  - `crates/printdex-app/src/app.rs` (handle the event)
  - `locales/en/main.ftl`
- Test: `crates/printdex-core/tests/it/supervise.rs` (add `mod supervise;` to `tests/it/main.rs`)

**Interfaces:**
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)] pub enum TaskName { SidecarWriter, Thumbnails, Monitor, BackupScheduler, Sync }`, exported from `printdex_core`.
  - `EngineEvent::TaskStopped(TaskName)`.
  - `pub(crate) fn spawn_supervised<F, Fut>(name: TaskName, events: broadcast::Sender<EngineEvent>, make: F) -> tokio::task::JoinHandle<()> where F: FnMut() -> Fut + Send + 'static, Fut: Future<Output = ()> + Send + 'static`.
  - Each run is wrapped in `diagnostics::recovering_future`. A normal return ends supervision. A panic is logged at `error`, followed by a restart after a backoff of 1 s, 5 s and then 30 s. After the 3rd panic within 10 minutes it emits `TaskStopped(name)` and ends.
  - Backoff durations are `pub(crate) const RESTART_BACKOFF: [Duration; 3]`. Tests use tokio's paused time.
  - Test-only: `faults::arm_panic(task: TaskName, times: usize) -> FaultGuard` and `pub fn checkpoint(task: TaskName)`. `checkpoint` panics while armed and is a no-op without the feature. Each supervised loop calls `checkpoint` once per iteration.
- State that must survive a restart lives outside the run future:
  - the sidecar writer's queue receiver goes in `Arc<tokio::sync::Mutex<…>>`
  - the thumbnail queue stays in `inner`
  - the monitor is re-entered with its config

- [ ] **Step 1: Write failing tests in `tests/it/supervise.rs`.**
  - `sidecar_writer_survives_a_panic`: `faults::arm_panic(TaskName::SidecarWriter, 1)`, edit a tag, `flush_sidecars()`. Assert the sidecar file was written and no `TaskStopped` was received.
  - `gives_up_after_three_panics` (`#[tokio::test(start_paused = true)]`): `arm_panic(TaskName::Thumbnails, usize::MAX)`, trigger thumbnail work, advance time past 36 s. Assert exactly one `TaskStopped(TaskName::Thumbnails)` and that the engine still answers `search`.
  - `monitor_restarts_after_panic`: arm 1 panic for `Monitor`. A file added to the library afterwards still produces a scan (reuse the helpers in `tests/it/monitor.rs`).
- [ ] **Step 2: Run** `cargo test -p printdex-core --test it supervise`. Expected: FAIL to compile.
- [ ] **Step 3: Implement `supervise.rs` and the panic fault.** Route all five spawn sites through `spawn_supervised`.
- [ ] **Step 4: Wire the app.** `EngineEvent::TaskStopped(t)` shows a toast that stays until it is dismissed, with the key `task-stopped` and the text "{ $task } stopped after repeated errors. Restart Printdex to try again." It offers a "Report a bug" action, the same one the crash notice uses.
  - Task labels: `task-sidecars` "Saving sidecar files", `task-thumbnails` "Thumbnail rendering", `task-monitor` "Watching for changes", `task-backup` "Scheduled backups", `task-sync` "Sync".
  - App test `task_stopped_shows_toast` in `app.rs` tests.
- [ ] **Step 5: Run** `cargo test -p printdex-core --test it` and `cargo test -p printdex-app`. Expected: PASS.
- [ ] **Step 6: Commit** `feat(engine): restart background tasks after a panic`.

### Task 3: A broken data folder gets a clear message

**Files:**
- Modify:
  - `crates/printdex-core/src/error.rs`
  - `db/mod.rs:52-81`
  - `engine.rs` (`open_with_fs`, new `set_aside_database`)
- Modify:
  - `crates/printdex-app/src/app.rs` (`Boot`, `Message::Booted(Err)`)
  - `view/mod.rs:45`
  - `locales/en/main.ftl`
- Test: `crates/printdex-core/tests/it/open.rs` (new, add `mod open;`). App tests in `app.rs`.

**Interfaces:**
- Produces:
  - `CoreError::DataTooNew(PathBuf)`, `CoreError::DataCorrupt(PathBuf)`, `CoreError::DataReadOnly(PathBuf)`, `CoreError::DiskFull(PathBuf)`. The path is the database file.
  - `pub(crate) fn classify_open_error(e: CoreError, db: &Path) -> CoreError`. It maps:
    - sqlx `MigrateError::VersionMissing`/`VersionTooNew`-style errors to `DataTooNew`
    - SQLite primary codes 11 (`CORRUPT`) and 26 (`NOTADB`) to `DataCorrupt`
    - codes 8 (`READONLY`) and 14 (`CANTOPEN`, when the file or folder isn't writable) to `DataReadOnly`
    - code 13 (`FULL`) and `io::ErrorKind::StorageFull` to `DiskFull`
    - anything else is returned unchanged
  - `Engine::open_with_fs` runs `PRAGMA quick_check(1)` only when opening fails with a non-classified sqlite error. Never on a healthy open, because it costs too much at 100k models.
  - `pub fn Engine::set_aside_database(data_dir: &Path) -> Result<PathBuf>`. It moves `printdex.db` (and `-wal`/`-shm` when present) to `printdex.db.broken-<ms>` (`-wal`/`-shm` keep their suffix after it) with `fsx::rename_no_replace`, and returns the new db path.
  - App: `Boot::Failed { dir: PathBuf, reason: BootFailure }` with `pub enum BootFailure { TooNew, Corrupt, ReadOnly, DiskFull, Other }`.

- [ ] **Step 1: Write failing core tests in `tests/it/open.rs`.**
  - `newer_schema_is_refused_untouched`: open an engine, close it, then insert a `_sqlx_migrations` row with version `9999`, success `1`, and a dummy checksum (use `rusqlite`-free raw sqlx on the file). Hash the db file, then reopen. Assert `Err(CoreError::DataTooNew(_))` and that the hash is unchanged.
  - `corrupt_db_is_reported_untouched`: write 4096 bytes of `0xAB` to `data/printdex.db` and open. Assert `DataCorrupt` and unchanged bytes.
  - `read_only_data_folder_is_reported` (`#[cfg(unix)]`): create the db, close it, then `chmod 0o555` the folder and `0o444` the file. Skip with an early return when running as root (`rustix::process::geteuid().is_root()`). Assert `DataReadOnly`.
  - `classify_maps_disk_full`: a unit test in `engine.rs` or `db/mod.rs` tests. Assert `classify_open_error(CoreError::Io{ source: io::Error::from(io::ErrorKind::StorageFull), .. })` gives `DiskFull`.
  - `set_aside_keeps_old_file`: corrupt the db, call `set_aside_database`, and assert the returned path exists with the same bytes and `printdex.db` is gone. The next `Engine::open` then succeeds with an empty library.
- [ ] **Step 2: Run** `cargo test -p printdex-core --test it open`. Expected: FAIL.
- [ ] **Step 3: Implement the variants, `classify_open_error` and `set_aside_database`.**
- [ ] **Step 4: App.** `Booted(Err)` maps the error to a `BootFailure`, and the failed screen shows a title, one line of explanation, and actions:
  - TooNew: "This data folder was last used by a newer Printdex. Update Printdex to open it; nothing was changed." Action: Show folder.
  - Corrupt: "Printdex's database in { $path } is damaged." Actions: "Set it aside and start fresh" (calls `set_aside_database`, then boots again; the toast says where the old file is and that a data backup can be restored from Settings › Backup), Show folder, Report a bug.
  - ReadOnly: "Printdex can't write to { $path }. Check the folder's permissions." Action: Show folder.
  - DiskFull: "The disk holding { $path } is full. Free some space, then try again." Actions: Try again (boots again), Show folder.
  - Other: the current `app-open-failed` text, plus Report a bug.
  - App tests: `boot_failure_reasons` (each error maps to its reason) and `set_aside_reboots` (on a temp dir with a corrupt db, the message sequence ends in `Boot::Ready`).
- [ ] **Step 5: Run** core `it open`, `-p printdex-app`. Expected: PASS.
- [ ] **Step 6: Screenshot** the Corrupt screen (light) to `docs/screenshots/hardening/corrupt-db.png` (see the project memory "cloud-app-screenshots").
- [ ] **Step 7: Commit** `feat: clear messages for a damaged, newer or unwritable data folder`.

### Task 4: Panic lints in printdex-core

**Files:**
- Modify: `crates/printdex-core/src/lib.rs` (crate attribute), and every file the lints flag (about 85 index, 32 slice and 42 cast sites, plus 5 `expect`)

**Interfaces:**
- Produces: the core crate attribute
  `#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::cast_possible_truncation, clippy::cast_sign_loss, clippy::cast_possible_wrap))]`.
  The same attribute is used in Tasks 5 and 6. Integration-test crates are unaffected.

- [ ] **Step 1: Add the attribute.** Run `cargo clippy -p printdex-core --lib -- -D warnings` and save the list of findings in the commit message body (as a count per lint).
- [ ] **Step 2: Fix every finding.** Prefer handling:
  - `.get()` with an error or skip
  - `try_from` with saturation (`u32::try_from(x).unwrap_or(u32::MAX)`)
  - `split_at_checked`
  - `?` on `CoreError`

  Keep the code where it is provably safe, and put `#[expect(clippy::<lint>, reason = "<why it cannot panic>")]` on the smallest item. The reason must state the invariant (e.g. "i < len checked on line above").

  In `sync/log.rs`:
  - `zstd::bulk::compress(..).expect(..)` becomes a returned `CoreError`.
  - The two `serde_json::to_*` expects stay as `#[expect]` with the reason "plain structs with String keys always serialize".
- [ ] **Step 2b: Write a test for each handled site whose new behavior is reachable** (input that used to panic now returns an error), next to the existing tests of that module. If a site has no reachable bad input, it gets `#[expect]` instead of handling.
- [ ] **Step 3: Run** `cargo clippy -p printdex-core --all-targets -- -D warnings` and `cargo test -p printdex-core`. Expected: clean and PASS.
- [ ] **Step 4: Commit** `refactor(core): no unchecked panics outside tests`.

### Task 5: Panic lints in printdex-app and printdex-cli

**Files:**
- Modify:
  - `crates/printdex-app/src/main.rs` and `crates/printdex-cli/src/main.rs` (crate attribute from Task 4)
  - every flagged file (about 31 index, 11 slice and 53 cast sites in the app, and 6, 5 and 4 in the CLI)
  - the one `panic!`/`unreachable!` in each

**Interfaces:**
- Consumes: the crate attribute from Task 4.

- [ ] **Step 1: Add the attribute to both crates and list the findings.**
- [ ] **Step 2: Fix or `#[expect]` each finding,** with the same rules as Task 4. UI geometry casts (`f32` to `u32` sizes) saturate and clamp to at least 1. Add tests for reachable handled sites in the module's existing tests.
- [ ] **Step 3: Run** clippy `-D warnings` and `cargo test -p printdex-app -p printdex-cli`. Expected: PASS.
- [ ] **Step 4: Commit** `refactor(app,cli): no unchecked panics outside tests`.

### Task 6: Odd state in the UI and degenerate meshes

**Files:**
- Modify:
  - `crates/printdex-mesh/src/lib.rs`, `crates/printdex-render/src/lib.rs` (crate attribute from Task 4, and fix findings)
  - `printdex-render/src/raster.rs` (`Sphere::of`)
  - `viewer.rs` (`home_view`, `project`)
  - `camera.rs` (`Fit::new`, `Fit::on_bed`, `Fit::orbit`)
  - `crates/printdex-app/src/app.rs` (selection handling)
- Test: unit tests in `raster.rs`, `camera.rs` and `viewer.rs`, and app tests in `app.rs`.

**Interfaces:**
- Consumes: the crate attribute from Task 4.
- Produces: every public function in `camera.rs`/`viewer.rs` returns finite values for any finite or zero-size input, and `raster::render` returns an image of the requested size for any mesh.

- [ ] **Step 1: Write failing tests.**
  - Render:
    - `degenerate_meshes_render`: for each mesh (empty; 3 equal points; one triangle; a triangle with a NaN coordinate; points at ±1e30), `raster::render(&mesh, &RenderOptions::default())` returns an image of the default size without panicking.
    - `fits_are_finite`: `Fit::new` with `min == max`, `width = 0`, `height = 0` and `margin = 0.5` has all fields finite. Same for `on_bed` and `orbit(radius = 0.0, zoom = 0.0)`.
    - `home_view_of_point_mesh_is_finite`: `home_view(Sphere { center: Vec3::ZERO, radius: 0.0 }, Bounds { min: [0.;3], max: [0.;3] }, 0.0, 0.0)` has a finite view.
  - App:
    - `deleted_selected_model_clears_selection`: select a model, then deliver `ModelData`/`Model` results of `Ok(None)` (and a `LibraryRemoved` event for its library). `selected` becomes `None` and `app.view()` builds.
    - `view_builds_for_odd_states`: `view()` builds with zero libraries, with a model that has zero parts, and after `Message::WindowResized(Size::new(1.0, 1.0))` (use the existing resize message name).
- [ ] **Step 2: Run them.** Expected: the failing cases (panic or NaN) fail. If a case already passes, keep it as a regression test and note that in the report.
- [ ] **Step 3: Fix.**
  - Clamp radii and sizes with `.max(1e-3)` / `.max(1.0)` where they divide.
  - Ignore non-finite vertices in `Sphere::of` (via `Mesh::bounds`, which already filters).
  - Clear the selection when its model or library goes away.
  - Then add the crate attribute to mesh and render, and fix or `#[expect]` the findings by the Task 4 rules. Loop indices bounded by `len()` get `#[expect(clippy::indexing_slicing, reason = "...")]` on the function.
- [ ] **Step 4: Run** clippy `-D warnings` on the workspace, then `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `fix: degenerate meshes and vanished selections never panic`.

### Task 7: Whole-branch check and PR

- [ ] **Step 1:** Run all gate checks and `cargo run -p printdex-cli -- bench run` against the 20k budget, with the same command the CI bench job uses.
- [ ] **Step 2:** Push and open a PR into `dev` that closes the pass 2 issue and refs #143. Attach the corrupt-db screenshot, label it `ci: full`, assign wolfedme, and watch CI until it is green on all three OSes.
