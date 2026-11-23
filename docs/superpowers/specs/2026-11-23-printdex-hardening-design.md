# Printdex hardening and optimization cycle — design

Date: 2026-11-23 · Status: draft, for Max's review · Thread: "Harden Printdex against edge cases"

## Intent

A routine hardening and optimization cycle, started by Max on 2026-11-23. Nothing has
broken and no one uses Printdex yet. The goal is to find the common and rare edge cases
across the whole app (scanning, file readers, database, file actions, backup, restore,
sync, UI) that Printdex doesn't handle yet, and fix them before other people's libraries
depend on the app.

Because no one uses it yet, fixes can change the database schema, sidecar format and
on-disk layout in place. No migration or compatibility work is needed for older
installs. (Migrations still have to work for a fresh database, and the existing migration
chain stays. Editing old migrations is allowed if that turns out to be simpler.)

## Goals

1. **No silent data loss.** The app never overwrites, deletes or drops a user's model
   file, sidecar, print log entry or photo without the user asking, in any order of
   crashes, failures and concurrent changes it can reasonably meet.
2. **No crash without a trace.** Every panic or startup failure leaves a readable record
   on disk and, where possible, a message in the app instead of a vanished window.
3. **Hostile environments degrade gracefully.** NAS disconnects, permission errors, a
   full disk, odd file names, case-insensitive drives and a second running copy of the
   app each produce a clear state or message, never corruption or a hang.
4. **Bad or huge input files are contained.** One broken or enormous file never takes
   down a scan, the thumbnail workers, the viewer or the app.
5. **Stays fast at scale.** Behaviour and speed are checked at 100k+ models, not only
   at the 4k-model CI bench.

Every fix ships with a test that reproduces the problem first (it must fail without the
fix).

## Non-goals

- New features. A missing feature only counts if its absence causes one of the failures
  above. For example, a crash log counts, and a log viewer in the UI doesn't. The one
  exception is "Report a bug" (pass 2), which Max asked for.
- Telemetry or crash uploads. Records stay on the user's machine (local-first).
- Rewriting subsystems that already pass the sweep.
- Upstream fixes to iced, rustic or other dependencies, beyond working around them.
- Windows Mica (#48) and render quality (#123). Both stay out of this cycle.

## Anti-goals (a failure even if it technically works)

- Making things "safe" by refusing too much. For example, a scan that gives up on a
  whole library because one folder is unreadable, or file actions that refuse whenever
  anything looks unusual.
- Error dialogs for things the app can recover from on its own.
- Slower normal operation. Hardening must not regress the bench budgets in
  `bench/baseline.json`, and fsync and similar costs belong only where data is at stake.
- Any `unsafe` code (project rule). Platform calls go through safe crate APIs.

## Constraints

- No `unsafe` in Printdex crates (`unsafe_code = "forbid"`). Dependencies must be
  GPL-3.0-compatible and pass cargo-deny.
- Three OSes: Linux is the primary test platform. Windows and macOS behaviour (case
  insensitivity, rename semantics, path length, reserved names) must be covered by
  tests that run on those CI runners or by OS-independent logic tests.
- CI minutes are limited: PRs run Linux only, and Windows and macOS run on `dev`/`main`
  or with the `ci: full` label. Any new slow tests (fuzzing, 100k-model runs) are
  opt-in or `#[ignore]`d with a documented command, not part of every PR run.
- Feature PRs target `dev`. Issues are closed by hand.
- Every piece of work gets a GitHub issue (Max's workflow), plus screenshots for
  user-visible changes.

## Approaches considered

1. **Area by area.** Audit and fix one subsystem at a time, one PR per area. It's easy
   to follow, but a cross-cutting failure like a full disk gets fixed piecemeal and
   inconsistently.
2. **By kind of failure (chosen by Max, 2026-11-23).** Work through failure classes
   in order of severity, checking each class across the whole app, with one PR per
   class and a reproducing test per fix. The worst problems get fixed first, and each
   fix covers every place the failure can hit.
3. **Tests first.** Build random-input and fault-injection tests across everything, then
   fix whatever they find. It finds surprises, but the result is less of a reviewed
   catalogue and harder to call done.

Approach 2 still uses some of approach 3: each class gets fault injection or random input
where that is the cheapest way to cover it.

## The five passes

Each pass is one PR into `dev` (split in two if it grows past what can be reviewed). The
order is fixed: later passes build on the shared pieces from earlier ones.

**Stopping rule (builder's call, 2026-11-23):** a pass is done when every **Known** item
is fixed and every **Sweep** item has been checked and is either fixed or logged as a
follow-up issue. An edge case found mid-pass goes into the current PR if it belongs to
that pass's class and is a small fix. Anything else becomes a follow-up issue. There is
no deadline (Printdex has none), and quality wins over speed.

**Order of the work (builder's call):** the log file and panic hook from pass 2 are built
first, as the opening commit of pass 1, so that every later fix can report what it
contains. If a pass's PR is still waiting for review when the next one starts, the next
branch is stacked on it rather than waiting. Under each
pass, **Known** lists what the recon on 2026-11-23 already found (verified in code),
and **Sweep** lists where to look next. The builder adds to both as they go.

### Pass 1 — Data loss and corruption

Known:
- **Rename can overwrite on case-insensitive drives.** `move_models`
  (`printdex-core/src/file_ops.rs:209`) rejects two models with the same target name,
  but compares case-sensitively. Moving `A/Benchy.stl` and `B/benchy.stl` into one
  folder on macOS or Windows passes every check. The second `std::fs::rename` then
  replaces the first file, and `rename` replaces existing files on every OS.
- **Check-then-rename race.** `execute` checks that each target is free
  (`file_ops.rs:410`), then renames later on a blocking thread. A file created in
  between (by the user, a sync client or a slicer) gets silently overwritten. Fix: a
  rename that refuses to replace (no-replace rename), with the existing checks kept
  for good error messages.
- **Cross-drive move loses symlinks and checks sizes only.** `copy_tree` skips
  symlinks and special files, and `tree_listing` only compares regular-file names and
  sizes. The source is then trashed, so symlinks inside a model folder are lost, and a
  corrupted copy of the right size passes. Fix: refuse to move trees that hold symlinks
  (or recreate them), and verify by content hash (xxh3 is already a dependency).
- **Interrupted restore can drop restored photos.** `apply_staged`
  (`backup/restore.rs:154`) moves the old data aside, then moves the staged database
  in, then the staged photos. A crash after the database move but before the photos
  move leaves a staging folder that has `READY` but no database. On the next start that
  looks like an unfinished restore, so the folder and the restored photos in it are
  deleted. Fix: make the swap resumable, for example with a journal of the steps or by
  treating "READY present" as "finish the swap" whatever is still in the folder.
- **Atomic writes are done five different ways.** Sidecars, thumbnails, sync store,
  settings and apps.toml each write a temporary file and rename it, with different
  rigour: only the sync store fsyncs, and settings and apps use a fixed `.tmp` name that
  two running copies would share. Fix: one shared durable-write helper with a unique
  temporary name, an fsync of the file (and the parent folder on Unix) for user data,
  and cleanup of stale temporary files at scan time.
- **A broken settings file is replaced by defaults.** `Settings::load` falls back to
  defaults on a parse error, and the next save overwrites the user's file. Fix: keep the
  broken file as `settings.toml.broken-<time>` before writing a new one, and do the same
  for apps.toml and broken sidecars.

Sweep: every `remove_*`, `rename`, `trash::*` and `DELETE FROM` path, file-action undo
after an external change, the sidecar writer racing a rename or move, the missing-model
grace period (30 days) when a share stays offline longer, sync merge with clock skew,
and backup prune (`forget`) keeping what the user needs.

Done means each destructive path has a test that injects failure or concurrency at
every step, and none of them loses data.

### Pass 2 — Crashes and panics

Known:
- **No panic record.** There's no `panic::set_hook` and no log file. Logs go to stderr,
  which a Windows GUI app or a desktop-launched Linux/macOS app throws away.
- `catch_unwind` exists only around GPU thumbnails and the CPU worker pool (`cpu.rs`).
  A panic in a tokio task or in iced's update loop takes the app down silently.

To build:
- A rolling log file in the data folder (`logs/printdex.log`, a few MB, kept for 7 days,
  through `tracing-appender` or similar), always on.
- A panic hook that writes the panic message, location, backtrace, version and OS to
  `logs/crash-<time>.txt`. On the next start, the app shows a small notice that it
  closed unexpectedly last time, with a "Show file" action. The notice adds no upload
  and no dialog loop.
- **Report a bug** (Max asked for it, 2026-11-23). It sits in Settings › About and in
  the crash notice, and offers two actions:
  - *Open a GitHub issue*: opens the browser on a new-issue page prefilled with the
    version, OS, renderer (GPU or CPU) and the last crash's message and location.
  - *Save diagnostics…*: saves a zip of the recent logs, crash files and a system
    summary, for people without a GitHub account.

  Nothing is sent automatically. Home-folder and library paths in what gets shared are
  replaced with placeholders. Pending Max's pick: GitHub issue, diagnostics file, or
  both (both recommended).
- An audit of every non-test `unwrap`, `expect`, slice index and `as` cast in app and
  core code. Each one is either proven safe with a comment or replaced with handling.
  There are about 400 `unwrap` calls in total, most of them in tests.
- Engine background tasks (monitor, sidecar writer, backup scheduler, sync) are
  supervised: a panic is logged, and the task restarts or reports a stopped state the UI
  shows, instead of dying silently.

Sweep: startup with a broken data folder (unreadable, read-only, on a full disk, a
database from a newer build whose migrations are unknown, a corrupt database file),
iced update and view with empty or odd state (zero libraries, a model with zero parts,
a deleted model still selected), window resize to tiny sizes, and the 3D viewer with
degenerate meshes (all points equal, NaN after transforms, a single triangle).

### Pass 3 — Hostile environments

Known:
- **Two copies of the app on one data folder.** Nothing stops it. Both run the monitor,
  sidecar writer, backup scheduler and sync, and settings saves race. Fix: an exclusive
  lock file in the data folder, taken through a safe crate such as `fs4` or `fd-lock`.
  A second launch says Printdex is already running and exits.
- **Data folder on a network share.** SQLite WAL is unsafe on SMB and NFS. Fix: detect
  it with the existing `volume.rs` and refuse, or warn and use a non-WAL mode. Which
  of these is left to the builder.
- **Non-UTF-8 file names are skipped silently.** The walk records them as skipped
  (`walk.rs:257`), and the folder watcher drops them too (`monitor.rs:249`), but the user
  is never told. Fix: count them in the scan summary and list them in the library's
  details.
- **Unicode normalization.** macOS can return decomposed (NFD) names and other systems
  composed (NFC) ones. Nothing in core normalizes. Check whether identity, sidecars or
  sync compare `rel_path` across installs or OSes, and normalize at the comparison
  points if so (not on disk).
- **Full disk** is only handled in backup and sync (`StorageFull`). Database writes,
  sidecar writes, thumbnails, photo import and file copies must report it clearly and
  leave nothing half-written. The thumbnail cache must stop growing, not fail per file.

Sweep: NAS drop mid-scan, mid-move, mid-backup, mid-restore and mid-thumbnail (with
`ChaosFs` extended to inject errors and stalls in the write paths, not only reads),
read-only libraries (file actions disabled with a reason), permission denied on a
subfolder, Windows paths over 260 characters, reserved names and trailing dots and
spaces, a library inside another library or pointing at the same folder twice, a
library root that becomes a file, the system clock jumping backwards (catalogue numbers,
the hybrid clock in sync, the backup scheduler), the keyring being unavailable, and the
OS trash being unavailable (Linux without a trash folder, network shares).

### Pass 4 — Bad and huge files

Known:
- The mesh readers already have limits and "never panics" tests. The default limits
  allow 50M triangles and 2 GiB per 3MF entry. Several thumbnail workers plus the 3D
  viewer can each load a mesh that size at once, which adds up to many GB of RAM. Fix:
  a shared memory budget across concurrent mesh loads, and a separate lower limit for
  the interactive viewer, which shows a "too large to preview" state.
- Photos up to 25 MB are accepted, but the app decodes them with iced's
  `image::Handle::from_path` with no pixel limit. A small but huge-dimension JPEG
  (decompression bomb) can use gigabytes. Fix: check dimensions at import and decode
  display copies at a bounded size.
- Thumbnail decode caps allocation at 512 MiB per image. Check how many decodes can
  run at once.

Sweep: zip bombs and deeply nested or recursive 3MF components (depth is capped at 16,
so check total component expansion), OBJ with huge or negative indices and missing
`.mtl` files, STL with the wrong extension or a 0-byte file, files that change size
while being read (NAS write in progress), sidecars with valid TOML but invalid values
(unknown UUIDs, negative counts, very long strings), and a random-input harness for
sidecar and settings parsing next to the existing mesh one.

### Pass 5 — Speed at scale

Known: the CI baseline uses 3,967 models. The 100k numbers in project memory (search
p95 ~5 ms, unchanged rescan ~3 s) come from one manual run in milestone 1 and predate
the later milestones.

Targets (builder's call, for Max to check): **100k models is the target**, and it must
meet the same budgets as `bench/baseline.json`: start to usable ≤ 1 s, search p95
≤ 50 ms, subtree rescan ≤ 200 ms, unchanged rescan fingerprints 0, idle CPU ≤ 1 %. RSS
gets a 100k budget of **≤ 400 MB** (the 250 MB budget is for the 4k CI library). In the
app, scrolling and typing in search stay at 60 fps or better on the GPU renderer.
**250k is measured and reported only.** A miss at 250k becomes a follow-up issue, not a
blocker. If a 100k budget can only be met with architecture work (for example paging the
grid), the pass stops and brings that to Max as its own issue instead of doing it inside
this cycle.

To do:
- Re-run `printdex bench` at 100k and 250k models (local and through `ChaosFs` latency
  for NAS), with the results recorded in the PR. The runs are not part of PR CI.
- Profile the app at 100k: startup to a filled grid, scroll, search typing, selecting
  all and running bulk actions on 10k models, filter chips, the sidebar counts, sync and
  backup over the full set.
- Fix the top hotspots found, and add budgets to `bench/baseline.json` for anything new
  worth guarding.
- Check memory at 100k (RSS budget 250 MB), including thumbnail cache handles held by
  the UI.

## Shared pieces built along the way

- `durable_write(path, bytes)` helper (pass 1), used by every writer.
- No-replace rename helper (pass 1), used by file actions and restore. It goes through
  a safe crate API on each OS. If an OS or filesystem has no no-replace rename (some
  SMB shares), the fallback is to check, rename and verify, and the action refuses if
  the target appeared meanwhile.
- `ChaosFs` write-path faults: errors, stalls, `StorageFull`, disappearing roots
  (passes 1 and 3).
- Log file and panic hook (designed in pass 2, built first in pass 1), used by later passes to report contained failures.

## Tracking

One epic issue "Hardening and optimization cycle" with five sub-issues, one per pass.
Each newly found edge case is listed in its pass's issue as a checklist item, so the
issue doubles as the catalogue. Findings that turn out to be features rather than
hardening go to new issues outside the epic instead of being built.

## Left to the builder

- The exact crates for the log file, the lock file and no-replace rename (safe APIs,
  GPL-compatible, cargo-deny clean).
- Whether a network-share data folder is refused or runs in a degraded mode.
- How the swap in restore is made resumable.
- Splitting a pass into two PRs when it gets large.
- Which `unwrap`s are proven safe rather than replaced.
- The wording of the UI messages (in the project's existing voice, through `i18n.rs`).
