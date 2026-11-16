# Printdex Foundation (Milestone 0) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Cargo workspace with a UI-free async engine that registers library folders, scans them into SQLite (models, parts, assets, fingerprints), answers full-text searches, and is driven by a `printdex` CLI that also generates and benchmarks a synthetic 100k-part library, all checked by CI on Linux, Windows and macOS.

**Architecture:** `printdex-core` owns everything: a parallel directory walk (blocking, run via `spawn_blocking`), partial-content fingerprints computed on a rayon pool, a pure folder-to-model grouping function, and a sqlx/SQLite store with FTS5. An `Engine` handle exposes async commands/queries and a `broadcast` event stream. `printdex-cli` is a thin clap front end over `Engine`. No UI, mesh or render crates yet; they arrive in milestones 1–2.

**Tech Stack:** Rust 2024 edition, tokio 1.54, tokio-util 0.7, rayon 1.12, sqlx 0.9 (sqlite, runtime-tokio, macros, migrate, uuid), ignore 0.4 (parallel walker), xxhash-rust 0.8 (xxh3), uuid 1 (v7), thiserror 2, anyhow 1, tracing 0.1 + tracing-subscriber 0.3, directories 6, clap 4 (derive), fastrand 2, dunce 1, tempfile 3 (dev).

**Spec:** `docs/superpowers/specs/2026-11-16-printdex-architecture-design.md` (this branch) and the product design `docs/superpowers/specs/2026-11-16-printdex-v1-design.md` (branch `docs/v1-design`, PR #1; copy at `/mnt/project-files/printdex/2026-11-16-printdex-v1-design.md`). Read §5–§7 of the v1 design and §5–§6, §11–§12 of the architecture design before starting.

**Deviation from spec (planner's call):** the architecture doc names `jwalk` for the directory walk. `jwalk` is now deprecated on crates.io ("Use `dua-core` instead"). This plan uses `ignore::WalkBuilder::build_parallel` (ripgrep's walker: MIT/Unlicense, widely used, parallel, configurable) with all gitignore filtering turned off.

## Global Constraints

- License of every crate: `GPL-3.0-only`. Every dependency must pass `cargo deny check licenses` with the allow list in Task 1 (GPL-3.0-compatible only).
- Platforms: CI builds and tests on `ubuntu-latest`, `windows-latest`, `macos-latest`. Linux is the primary platform.
- Fully offline: no network calls anywhere in this milestone.
- Never modify, move or delete files inside a library. The scanner only lists directories and reads file contents for fingerprints.
- All entity ids are UUID v7. Every table row has `created_at` and `updated_at` (unix milliseconds, `INTEGER`).
- Fingerprint = file size + xxh3-64 of the first and last 64 KiB (whole file if ≤ 128 KiB).
- Unchanged file = same relative path + size + mtime → never re-read.
- Missing files/models are marked missing (`missing_since`), never deleted from the DB.
- sqlx compile-time checked queries (`query!`/`query_as!`) with offline metadata in `.sqlx/` committed; CI builds with `SQLX_OFFLINE=true`.
- Engine errors are typed variants (codes + data), not English sentences, so a UI can translate them later.
- Relative paths are stored with `/` separators on every OS.
- Performance targets this milestone must report (not yet enforce): rescan of an unchanged library reads no file contents; search p95 < 50 ms on 100k parts.
- Only Max writes code to this repo besides Claude: no outside contributions (CONTRIBUTING.md says so).

## Review Focus

1. **Library root offline during rescan** (NAS unmounted, USB drive unplugged): a reasonable person expects their library to stay intact. Rescan must fail with `CoreError::LibraryOffline` and mark nothing missing. Test: Task 8, `rescan_of_offline_library_marks_nothing_missing`.
2. **Search text containing FTS5 syntax** (`"`, `*`, `-`, `:`, `(`, `AND`, `NEAR`): typing `dragon (v2)` or `"` must never error; it searches literally. Test: Task 7, `fts_query_escapes_syntax`, and Task 8, `search_with_syntax_chars_does_not_error`.
3. **Unusual file names**: non-ASCII names (`Drachen_Ü.stl`, `龍.3mf`), upper-case extensions (`.STL`, `.3MF`), and on Linux non-UTF-8 names. Expect: the first two are indexed and searchable; non-UTF-8 paths are skipped and counted, never crash. Tests: Task 3, `classifies_uppercase_extensions`, Task 3, `skips_non_utf8_names` (Linux only), Task 8, `finds_models_by_non_ascii_name`.
4. **Hostile directory trees**: symlink loops, unreadable subfolders, NAS junk folders (`@eaDir`, `#recycle`, `$RECYCLE.BIN`, `.Trashes`, `__MACOSX`, `System Volume Information`). Expect: the walk terminates, junk is ignored, unreadable folders are skipped and counted. Tests: Task 3, `does_not_follow_symlink_loops` (unix), `ignores_nas_junk_dirs`, `skips_unreadable_dirs` (unix).
5. **Files changing mid-scan** (deleted or truncated between walk and fingerprint, typical while a download/unzip is running): the file is skipped this round, the scan completes. Test: Task 2, `fingerprint_of_missing_file_is_io_error`, and Task 6, `scan_survives_file_deleted_before_fingerprint`.

---

## File Structure

```
Cargo.toml                         workspace, shared deps, lints, rust-version
deny.toml                          license allow list + advisories
rust-toolchain.toml                pins stable channel + rustfmt/clippy components
.sqlx/                             sqlx offline query metadata (generated, committed)
.github/workflows/ci.yml           fmt, clippy, test (3 OS), msrv, deny, bench-smoke
.github/pull_request_template.md   points to CONTRIBUTING.md
CONTRIBUTING.md                    "issues and ideas welcome; no outside PRs yet"
crates/printdex-core/
  Cargo.toml
  migrations/0001_init.sql         libraries, models, parts, assets, models_fts + triggers
  src/lib.rs                       module wiring + re-exports
  src/ids.rs                       LibraryId, ModelId, PartId, AssetId
  src/error.rs                     CoreError, Result
  src/time.rs                      now_ms()
  src/cpu.rs                       run_cpu(): rayon → async bridge
  src/fingerprint.rs               Fingerprint, fingerprint_file()
  src/walk.rs                      FileEntry, FileKind, walk_library()
  src/grouping.rs                  ModelGroup, group_models()
  src/db/mod.rs                    Db (writer + reader pools), open/migrate
  src/db/libraries.rs              library rows
  src/db/files.rs                  KnownFile load, ScanWrite apply
  src/db/search.rs                 to_fts_query(), search_models(), stats()
  src/scan.rs                      diff(), scan_library()
  src/events.rs                    EngineEvent, ScanPhase
  src/engine.rs                    Engine
  tests/engine.rs                  end-to-end engine tests on temp dirs
crates/printdex-cli/
  Cargo.toml
  src/main.rs                      clap commands: scan, search, stats, bench gen, bench run
  src/bench/gen.rs                 synthetic library generator
  src/bench/run.rs                 timing harness
  tests/cli.rs                     generator determinism + smoke run
```

---

### Task 1: Workspace, CI, licensing gate, contribution policy

**Files:**
- Create: `Cargo.toml`, `rust-toolchain.toml`, `deny.toml`, `.github/workflows/ci.yml`, `.github/pull_request_template.md`, `CONTRIBUTING.md`, `crates/printdex-core/Cargo.toml`, `crates/printdex-core/src/lib.rs`, `crates/printdex-cli/Cargo.toml`, `crates/printdex-cli/src/main.rs`
- Modify: `.gitignore` (add `/target`)

**Interfaces:**
- Produces: workspace members `crates/printdex-core` (lib `printdex_core`) and `crates/printdex-cli` (bin `printdex`); `[workspace.dependencies]` with the versions in the Tech Stack line; `[workspace.package]` with `edition = "2024"`, `license = "GPL-3.0-only"`, `rust-version = "1.88"`, `repository = "https://github.com/wolfedme/printdex"`; `[workspace.lints.clippy]` `all = "warn"`, `[workspace.lints.rust]` `unsafe_code = "forbid"`.

- [ ] **Step 1: Write the workspace and both crates.** `printdex-core/src/lib.rs` contains only a doc comment and `pub const VERSION: &str = env!("CARGO_PKG_VERSION");`. `printdex-cli/src/main.rs` prints `printdex {VERSION}` on `--version` via clap derive (one `Cli` struct, no subcommands yet). Add one test in `printdex-core/src/lib.rs`: `version_is_set` asserting `!VERSION.is_empty()`.
- [ ] **Step 2: Write `deny.toml`.** `[licenses] allow = ["MIT", "Apache-2.0", "Apache-2.0 WITH LLVM-exception", "BSD-2-Clause", "BSD-3-Clause", "ISC", "Zlib", "MPL-2.0", "Unicode-3.0", "Unlicense", "CC0-1.0", "BSL-1.0", "GPL-3.0-only", "GPL-3.0-or-later", "LGPL-2.1-or-later", "LGPL-3.0-or-later"]`, `confidence-threshold = 0.9`; `[advisories]` default (deny vulnerabilities); `[bans] multiple-versions = "warn"`. Workspace crates themselves must pass (they're GPL-3.0-only).
- [ ] **Step 3: Write `.github/workflows/ci.yml`** with jobs, all on push and pull_request, `env: SQLX_OFFLINE: "true"`, `CARGO_TERM_COLOR: always`, using `dtolnay/rust-toolchain` and `Swatinem/rust-cache@v2`:
  - `lint` (ubuntu): `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`.
  - `test` (matrix ubuntu/windows/macos): `cargo test --workspace`.
  - `msrv` (ubuntu): toolchain `1.88`, `cargo check --workspace --all-targets`.
  - `deny` (ubuntu): `EmbarkStudios/cargo-deny-action@v2`.
  - `bench-smoke` is added in Task 10.
- [ ] **Step 4: Write `CONTRIBUTING.md` and the PR template.** CONTRIBUTING says, in plain words: issues, bug reports and ideas are welcome; pull requests from outside contributors are not being accepted yet, so that licensing stays flexible; this may change later. The PR template is one line linking to CONTRIBUTING.md.
- [ ] **Step 5: Verify locally.**
  Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
  Expected: all pass, 1 test passed.
  Run: `cargo install cargo-deny --locked && cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`.
  Run: `rustup toolchain install 1.88 --profile minimal && cargo +1.88 check --workspace`. If it fails because a dependency needs newer Rust, raise `rust-version` (and the CI msrv toolchain) to the lowest version that passes, and note the reason in the commit message.
- [ ] **Step 6: Commit** `chore: scaffold workspace, CI, cargo-deny and contribution policy`.

---

### Task 2: Ids, errors, time, CPU bridge, fingerprints

**Files:**
- Create: `crates/printdex-core/src/{ids.rs,error.rs,time.rs,cpu.rs,fingerprint.rs}`
- Modify: `crates/printdex-core/src/lib.rs`, `crates/printdex-core/Cargo.toml`

**Interfaces:**
- Produces:
  - `ids.rs`: `LibraryId`, `ModelId`, `PartId`, `AssetId`, each `#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, sqlx::Type)] #[sqlx(transparent)] pub struct X(pub Uuid);` with `X::new() -> X` (UUID v7) and `Display` (hyphenated). Stored as SQLite `BLOB` (16 bytes) via sqlx's `uuid` feature.
  - `error.rs`: `pub enum CoreError { LibraryNotFound(LibraryId), LibraryPathInvalid(PathBuf), LibraryOverlaps { existing: PathBuf }, LibraryOffline(PathBuf), AlreadyScanning(LibraryId), Cancelled, Io { path: PathBuf, source: std::io::Error }, Db(#[from] sqlx::Error), Migrate(#[from] sqlx::migrate::MigrateError) }` (thiserror), `pub type Result<T, E = CoreError>`.
  - `time.rs`: `pub fn now_ms() -> i64`, `pub fn system_time_to_ns(t: SystemTime) -> i64` (negative for pre-epoch).
  - `cpu.rs`: `pub async fn run_cpu<T, F>(f: F) -> T where F: FnOnce() -> T + Send + 'static, T: Send + 'static` — `rayon::spawn` + `tokio::sync::oneshot`; a panic in `f` is resumed on the awaiting task (`std::panic::catch_unwind` + `resume_unwind`).
  - `fingerprint.rs`: `pub const SAMPLE_BYTES: u64 = 64 * 1024;` `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Fingerprint { pub size: u64, pub hash: u64 }`; `pub fn fingerprint_file(path: &Path) -> std::io::Result<Fingerprint>`.

- [ ] **Step 1: Write failing tests** in `fingerprint.rs` (use `tempfile`):
  - `small_file_hashes_whole_content`: 100-byte file; `fp.size == 100`, `fp.hash == xxh3_64(&bytes)`.
  - `boundary_128k_hashes_whole_content`: exactly 131072 bytes → `hash == xxh3_64(&all)`.
  - `large_file_hashes_head_and_tail_only`: 1 MiB file A; file B = A with byte 500_000 changed → same fingerprint; file C = A with last byte changed → different hash.
  - `size_is_part_of_identity`: two files with identical head/tail samples but different middle lengths → different `size`, so `!=`.
  - `empty_file`: size 0, `hash == xxh3_64(b"")`.
  - `fingerprint_of_missing_file_is_io_error`: nonexistent path → `Err(e)` with `e.kind() == NotFound`.
  - In `ids.rs`: `ids_are_v7_and_sortable`: two `ModelId::new()` in sequence → `a.0.get_version_num() == 7` and `a <= b`.
  - In `cpu.rs` (`#[tokio::test]`): `run_cpu_returns_value` (`run_cpu(|| 2 + 2).await == 4`) and `run_cpu_propagates_panic` (`#[should_panic(expected = "boom")]`).
- [ ] **Step 2: Run** `cargo test -p printdex-core` → FAIL (items not defined).
- [ ] **Step 3: Implement** the interfaces above. Fingerprint: read file length from the open handle's metadata; if `size <= 2 * SAMPLE_BYTES` stream the whole file into `xxh3::Xxh3`; else feed the first 64 KiB then seek to `size - 64 KiB` and feed the last 64 KiB. Reads use a fixed 64 KiB buffer (no full-file allocation).
- [ ] **Step 4: Run** `cargo test -p printdex-core` → PASS.
- [ ] **Step 5: Commit** `feat(core): ids, errors, cpu bridge and partial-content fingerprints`.

---

### Task 3: Directory walk and file classification

**Files:**
- Create: `crates/printdex-core/src/walk.rs`
- Modify: `crates/printdex-core/src/lib.rs`, `Cargo.toml` (add `ignore`)

**Interfaces:**
- Consumes: `CoreError`, `time::system_time_to_ns`.
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum PrintableFormat { Stl, ThreeMf, Obj }`
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum FileKind { Printable(PrintableFormat), Other }`
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub struct FileEntry { pub rel_path: String, pub size: u64, pub mtime_ns: i64, pub kind: FileKind }` — `rel_path` relative to the library root, `/`-separated, no leading `/`.
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub enum SkipReason { NonUtf8, Unreadable }`; `pub struct SkippedPath { pub path: PathBuf, pub reason: SkipReason }`
  - `#[derive(Debug, Default)] pub struct WalkOutput { pub files: Vec<FileEntry>, pub skipped: Vec<SkippedPath> }` with `files` sorted by `rel_path`.
  - `pub fn classify(file_name: &str) -> FileKind` — extension match, ASCII case-insensitive: `stl`→Stl, `3mf`→ThreeMf, `obj`→Obj, else Other.
  - `pub const IGNORED_DIR_NAMES: &[&str] = &["@eaDir", "#recycle", "#snapshot", "$RECYCLE.BIN", "System Volume Information", ".Trashes", "__MACOSX", "lost+found"];`
  - `pub fn walk_library(root: &Path, cancel: &CancellationToken) -> Result<WalkOutput>` — blocking; callers run it in `spawn_blocking`.

- [ ] **Step 1: Write failing tests** (temp dirs):
  - `classifies_uppercase_extensions`: `classify("A.STL") == Printable(Stl)`, `"b.3MF"` → ThreeMf, `"c.Obj"` → Obj, `"readme.md"` → Other, `"stl"` (no dot) → Other.
  - `walks_nested_files_with_forward_slashes`: create `a/b/c.stl` and `a/x.png` → `files` rel_paths `["a/b/c.stl", "a/x.png"]` with correct sizes; `mtime_ns > 0`.
  - `skips_hidden_entries_and_sidecars`: `.hidden/a.stl`, `m/.printdex.toml`, `m/.DS_Store` → none returned.
  - `ignores_nas_junk_dirs`: `@eaDir/x.stl`, `m/__MACOSX/y.stl`, `#recycle/z.stl` → none returned.
  - `does_not_follow_symlink_loops` (`#[cfg(unix)]`): `m/a.stl` plus symlink `m/loop -> ..` → returns exactly `["m/a.stl"]` and terminates.
  - `skips_unreadable_dirs` (`#[cfg(unix)]`, skip when running as root since root can read anything): `locked/` with mode 000 containing `x.stl`, plus `ok/a.stl` → files `["ok/a.stl"]`, `skipped` contains `locked` with `Unreadable`.
  - `skips_non_utf8_names` (`#[cfg(target_os = "linux")]`): file named with bytes `b"bad\xffname.stl"` → not in `files`, one `SkippedPath { reason: NonUtf8 }`.
  - `missing_root_is_library_offline`: nonexistent root → `Err(CoreError::LibraryOffline(_))`.
  - `cancelled_walk_returns_cancelled`: cancel token before calling → `Err(CoreError::Cancelled)`.
- [ ] **Step 2: Run** `cargo test -p printdex-core walk` → FAIL.
- [ ] **Step 3: Implement.** Check `root` is a readable directory first (`LibraryOffline` otherwise). `ignore::WalkBuilder::new(root)` with `standard_filters(false)`, `hidden(true)` (skip dot-entries), `follow_links(false)`, `filter_entry` rejecting `IGNORED_DIR_NAMES`; `build_parallel()` collecting into a `Mutex<WalkOutput>` (or per-thread vecs merged); check `cancel.is_cancelled()` in the visitor and `WalkState::Quit`. Use the entry's `metadata()` for size/mtime (one `stat` per file, no reads). Directory errors become `SkippedPath { Unreadable }`. Sort `files` at the end.
- [ ] **Step 4: Run** `cargo test -p printdex-core walk` → PASS.
- [ ] **Step 5: Commit** `feat(core): parallel library walk with file classification`.

---

### Task 4: Folder-to-model grouping

**Files:**
- Create: `crates/printdex-core/src/grouping.rs`
- Modify: `crates/printdex-core/src/lib.rs`

**Interfaces:**
- Consumes: `FileEntry`, `FileKind`.
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum ModelKind { Folder, SingleFile }` (stored as TEXT `'folder'` / `'file'`).
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub struct ModelGroup { pub rel_path: String, pub name: String, pub kind: ModelKind, pub parts: Vec<usize>, pub assets: Vec<usize> }` — `parts`/`assets` are indices into the input slice, ascending.
  - `pub const PART_FOLDER_NAMES: &[&str] = &["stl", "stls", "3mf", "obj", "files", "parts", "print files", "printfiles", "supported", "unsupported", "presupported", "pre-supported", "pre supported", "lychee", "chitubox"];` (ASCII case-insensitive match on the folder name).
  - `pub fn group_models(files: &[FileEntry]) -> Vec<ModelGroup>` — output sorted by `rel_path`.

The rule (v1 design §6, made precise; the scanner-override UI comes in a later milestone):

```
"printable dir" = a directory that directly contains ≥1 printable file
"has printables" = its subtree contains ≥1 printable file
"part folder"   = a directory whose name is in PART_FOLDER_NAMES

visit(dir, is_library_root):
  P = printable files directly in dir
  C = child dirs that have printables
  if is_library_root:
      each file in P → SingleFile model; visit each child in C
  else if P non-empty:
      if C contains any non-part folder:            # container with loose files
          each file in P → SingleFile model; visit each child in C
      else:
          Folder model rooted at dir (whole subtree)
  else (P empty):
      if C non-empty and every child in C is a part folder:
          Folder model rooted at dir (whole subtree)
      else:
          visit each child in C

Folder model: name = last path segment of dir; parts = all printable files in
the subtree; assets = all Other files in the subtree.
SingleFile model: rel_path = the file's rel_path; name = file stem; assets = [].
Other files not inside any Folder model are ignored.
```

- [ ] **Step 1: Write failing tests**, each building `FileEntry`s from a list of paths (helper `entries(&["..."])` that classifies by extension, size 1, mtime 1):
  - `folder_with_files_is_one_model`: `Dragon/a.stl, Dragon/b.3mf, Dragon/cover.jpg` → 1 Folder model `Dragon`, 2 parts, 1 asset.
  - `part_subfolders_merge_into_parent`: `Dragon/STLs/a.stl, Dragon/Supported/a_sup.stl, Dragon/readme.txt` → 1 Folder model `Dragon` with 2 parts, 1 asset.
  - `printable_dir_absorbs_any_subfolders_when_all_are_part_folders`: `Dragon/body.stl, Dragon/supported/body.stl` → 1 model `Dragon`, 2 parts.
  - `category_folders_are_not_models`: `Minis/Dragons/Red/a.stl, Minis/Dragons/Blue/b.stl` → 2 Folder models `Minis/Dragons/Red`, `Minis/Dragons/Blue`.
  - `loose_file_next_to_model_folders_is_single_file_model`: `Minis/goblin.stl, Minis/Orc/orc.stl` → SingleFile `Minis/goblin.stl` (name `goblin`) + Folder `Minis/Orc`.
  - `files_in_library_root_are_single_file_models`: `a.stl, Orc/orc.stl` → SingleFile `a.stl` + Folder `Orc`.
  - `folder_of_only_part_folders_without_name_match`: `Kit/Base/x.stl, Kit/Arm/y.stl` → 2 models `Kit/Base`, `Kit/Arm` (Base/Arm are not part-folder names).
  - `assets_outside_models_are_ignored`: `Minis/index.pdf, Minis/Orc/orc.stl` → 1 model `Minis/Orc` with 0 assets.
  - `part_folder_match_is_case_insensitive`: `X/STLS/a.stl` and `Y/Pre-Supported/b.stl` → models `X`, `Y`.
- [ ] **Step 2: Run** `cargo test -p printdex-core grouping` → FAIL.
- [ ] **Step 3: Implement** with a directory tree built from the sorted `rel_path`s (e.g. `BTreeMap<&str, DirNode>`), then the `visit` recursion above. Pure, no I/O.
- [ ] **Step 4: Run** `cargo test -p printdex-core grouping` → PASS.
- [ ] **Step 5: Commit** `feat(core): folder-to-model grouping rules`.

---

### Task 5: Database schema, pools and library rows

**Files:**
- Create: `crates/printdex-core/migrations/0001_init.sql`, `crates/printdex-core/src/db/mod.rs`, `crates/printdex-core/src/db/libraries.rs`, `.sqlx/` (generated)
- Modify: `crates/printdex-core/src/lib.rs`, `Cargo.toml` (sqlx features `["sqlite", "runtime-tokio", "macros", "migrate", "uuid"]`, `default-features = false`)

**Interfaces:**
- Consumes: ids, `CoreError`, `now_ms`.
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum LibraryKind { Local, Removable, Network }` (TEXT `'local'|'removable'|'network'`; always `Local` this milestone, detection comes in milestone 4).
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub struct Library { pub id: LibraryId, pub path: PathBuf, pub display_name: String, pub kind: LibraryKind, pub sidecar_enabled: bool, pub last_full_scan_at: Option<i64> }`
  - `#[derive(Clone)] pub struct Db { writer: SqlitePool, reader: SqlitePool }`
  - `impl Db { pub async fn open(path: &Path) -> Result<Db>; pub async fn insert_library(&self, lib: &Library) -> Result<()>; pub async fn libraries(&self) -> Result<Vec<Library>>; pub async fn library(&self, id: LibraryId) -> Result<Library> /* LibraryNotFound */; pub async fn set_last_full_scan(&self, id: LibraryId, at_ms: i64) -> Result<()> }`

Schema (`0001_init.sql`), exact:

```sql
CREATE TABLE libraries (
  id BLOB PRIMARY KEY, path TEXT NOT NULL UNIQUE, display_name TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'local', sidecar_enabled INTEGER NOT NULL DEFAULT 1,
  last_full_scan_at INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE models (
  id BLOB PRIMARY KEY, library_id BLOB NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
  rel_path TEXT NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL,
  missing_since INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE (library_id, rel_path));
CREATE TABLE parts (
  id BLOB PRIMARY KEY, model_id BLOB NOT NULL REFERENCES models(id) ON DELETE CASCADE,
  library_id BLOB NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
  rel_path TEXT NOT NULL, format TEXT NOT NULL, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL,
  fp_hash INTEGER, missing_since INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE (library_id, rel_path));
CREATE INDEX parts_fp ON parts(size, fp_hash);
CREATE INDEX parts_model ON parts(model_id);
CREATE TABLE assets (
  id BLOB PRIMARY KEY, model_id BLOB NOT NULL REFERENCES models(id) ON DELETE CASCADE,
  library_id BLOB NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
  rel_path TEXT NOT NULL, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL,
  missing_since INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE (library_id, rel_path));
CREATE VIRTUAL TABLE models_fts USING fts5(
  name, rel_path, content='models', content_rowid='rowid',
  tokenize = "unicode61 remove_diacritics 2");
CREATE TRIGGER models_ai AFTER INSERT ON models BEGIN
  INSERT INTO models_fts(rowid, name, rel_path) VALUES (new.rowid, new.name, new.rel_path); END;
CREATE TRIGGER models_ad AFTER DELETE ON models BEGIN
  INSERT INTO models_fts(models_fts, rowid, name, rel_path) VALUES ('delete', old.rowid, old.name, old.rel_path); END;
CREATE TRIGGER models_au AFTER UPDATE OF name, rel_path ON models BEGIN
  INSERT INTO models_fts(models_fts, rowid, name, rel_path) VALUES ('delete', old.rowid, old.name, old.rel_path);
  INSERT INTO models_fts(rowid, name, rel_path) VALUES (new.rowid, new.name, new.rel_path); END;
```

(`models` keeps SQLite's implicit `rowid` alongside the BLOB primary key; tags, notes and recipe text join the FTS index in milestone 3.)

- [ ] **Step 1: Write failing tests** in `db/libraries.rs` (`#[tokio::test]`, DB file in a `tempfile::TempDir`):
  - `open_runs_migrations_and_enables_wal`: after `Db::open`, `PRAGMA journal_mode` on the writer returns `wal`; `PRAGMA foreign_keys` returns `1`.
  - `open_is_idempotent`: opening the same path twice succeeds.
  - `insert_and_list_library`: inserted library round-trips through `libraries()` and `library(id)` field-for-field.
  - `unknown_library_is_not_found`: `library(LibraryId::new())` → `Err(LibraryNotFound(_))`.
  - `fts_trigger_indexes_models`: insert a library and a model row (`name = "Red Dragon"`) with raw SQL, then `SELECT count(*) FROM models_fts WHERE models_fts MATCH 'dragon'` → 1.
- [ ] **Step 2: Run** `cargo test -p printdex-core db` → FAIL.
- [ ] **Step 3: Implement.** `SqliteConnectOptions::new().filename(path).create_if_missing(true).journal_mode(Wal).synchronous(Normal).foreign_keys(true).busy_timeout(5s)`; writer pool `max_connections(1)`, reader pool `max_connections(4)` opened `read_only(true)` after migrations ran on the writer (`sqlx::migrate!("./migrations")`). Use `query!`/`query_as!` with `id as "id: LibraryId"` overrides. Paths stored as UTF-8 text (`to_str()`, already guaranteed by Engine::add_library in Task 8).
- [ ] **Step 4: Generate offline metadata.** `cargo install sqlx-cli --no-default-features --features sqlite --locked`; `export DATABASE_URL=sqlite://$PWD/target/dev.db; sqlx database create; sqlx migrate run --source crates/printdex-core/migrations; cargo sqlx prepare --workspace`. Add a short "Database" section to `CONTRIBUTING.md`'s maintainer notes (or `docs/dev.md`) with these exact commands.
- [ ] **Step 5: Run** `SQLX_OFFLINE=true cargo test -p printdex-core db` → PASS.
- [ ] **Step 6: Commit** `feat(core): sqlite schema, migrations and library store` (includes `.sqlx/`).

---

### Task 6: Scan pipeline (diff, fingerprint, group, write)

**Files:**
- Create: `crates/printdex-core/src/scan.rs`, `crates/printdex-core/src/db/files.rs`, `crates/printdex-core/src/events.rs`
- Modify: `crates/printdex-core/src/lib.rs`, `crates/printdex-core/src/db/mod.rs`, `.sqlx/`

**Interfaces:**
- Consumes: `walk_library`, `group_models`, `fingerprint_file`, `run_cpu`, `Db`, `Library`.
- Produces:
  - `events.rs`: `#[derive(Clone, Debug)] pub enum ScanPhase { Walking, Fingerprinting, Writing }`; `#[derive(Clone, Debug)] pub enum EngineEvent { LibraryAdded(Library), ScanStarted { library: LibraryId }, ScanProgress { library: LibraryId, phase: ScanPhase, done: u64, total: u64 }, ScanFinished { library: LibraryId, summary: ScanSummary }, ScanFailed { library: LibraryId, error: Arc<CoreError> } }`
  - `db/files.rs`: `#[derive(Clone, Debug)] pub struct KnownFile { pub rel_path: String, pub size: u64, pub mtime_ns: i64, pub fp_hash: Option<u64>, pub is_part: bool }`; `impl Db { pub async fn known_files(&self, lib: LibraryId) -> Result<Vec<KnownFile>>; pub async fn apply_scan(&self, lib: LibraryId, write: ScanWrite) -> Result<ApplyStats> }`
  - `scan.rs`:
    - `pub struct ScanWrite { pub files: Vec<FileEntry>, pub fingerprints: HashMap<usize, Fingerprint> /* index into files; only changed/new parts */, pub groups: Vec<ModelGroup> }`
    - `#[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct ApplyStats { pub models_added: u64, pub models_missing: u64, pub models_returned: u64 }`
    - `#[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct ScanSummary { pub files_seen: u64, pub fingerprinted: u64, pub skipped: u64, pub models_added: u64, pub models_missing: u64, pub models_returned: u64, pub duration_ms: u64 }`
    - `pub fn needs_fingerprint(known: &HashMap<&str, &KnownFile>, entry: &FileEntry) -> bool` — true for printable entries that are new, or whose size or mtime differ, or whose known `fp_hash` is `None`.
    - `pub(crate) async fn fingerprint_entries(root: &Path, files: &[FileEntry], which: &[usize]) -> (HashMap<usize, Fingerprint>, Vec<usize>)` — fingerprints `files[i]` for each `i` in `which` on the rayon pool; returns successes and the indices that failed.
    - `pub async fn scan_library(db: &Db, lib: &Library, events: &broadcast::Sender<EngineEvent>, cancel: CancellationToken) -> Result<ScanSummary>`

`apply_scan` semantics, in one writer transaction per 5,000 rows (the last batch also does the missing-marking):
- Upsert every model in `groups` by `(library_id, rel_path)`: keep the existing id, update `name`/`kind`, set `missing_since = NULL` (counts `models_returned` if it was set), new rows get `ModelId::new()` (counts `models_added`).
- Upsert every part and asset by `(library_id, rel_path)` pointing at its group's model id; keep ids; update `size`, `mtime_ns`, `format`, and `fp_hash` when a new fingerprint is present; clear `missing_since`.
- Every model/part/asset row of this library not touched in this scan and with `missing_since IS NULL` gets `missing_since = now` (models counted in `models_missing`). Nothing is deleted.
- `updated_at` changes only on rows whose values changed.

`scan_library` flow: emit `ScanStarted` → `spawn_blocking(walk_library)` (Walking) → `known_files` → select entries with `needs_fingerprint` → fingerprint them via `run_cpu` in chunks of 256 using `rayon` `par_iter` inside, emitting `ScanProgress` per chunk and checking `cancel` between chunks; a file whose fingerprint fails with `NotFound` or any I/O error is dropped from `files` and counted in `skipped` (logged with `tracing::warn!`) → `group_models` → `apply_scan` (Writing) → `set_last_full_scan` → `ScanFinished`. On error emit `ScanFailed` and return the error. Cancel → `CoreError::Cancelled`, no writes applied.

- [ ] **Step 1: Write failing tests** in `scan.rs` (temp library dir + temp DB, a library row inserted directly, a `broadcast::channel(256)`):
  - `first_scan_indexes_models_parts_assets`: tree `Dragon/STLs/a.stl, Dragon/cover.jpg, Orc/orc.3mf` → summary `files_seen == 3`, `fingerprinted == 2`, `models_added == 2`; DB has 2 models, 2 parts with non-null `fp_hash`, 1 asset.
  - `unchanged_rescan_reads_no_contents`: scan twice → second summary `fingerprinted == 0`, `models_added == 0`, `models_missing == 0`.
  - `touched_file_is_refingerprinted`: change `a.stl` content and size → second scan `fingerprinted == 1`; stored `fp_hash` changed.
  - `model_ids_are_stable_across_rescans`: the `Dragon` model id is identical after a rescan.
  - `deleted_folder_marks_model_missing_not_deleted`: remove `Orc/` → `models_missing == 1`; the `Orc` row still exists with `missing_since` set; its part also has `missing_since` set.
  - `restored_folder_returns`: put `Orc/` back → `models_returned == 1`, `missing_since` cleared, same id as before.
  - `scan_survives_file_deleted_before_fingerprint`: call the internal fingerprint stage with an entry whose file was removed after walking (test the helper `fingerprint_entries(root, entries) -> (HashMap<usize, Fingerprint>, Vec<usize /*failed*/>)`) → the entry is in `failed`; a full `scan_library` on the remaining tree succeeds.
  - `emits_started_progress_finished`: receiver sees `ScanStarted`, ≥1 `ScanProgress`, then `ScanFinished` with the returned summary.
  - `cancel_before_write_changes_nothing`: cancelled token → `Err(Cancelled)` and 0 models in DB.
- [ ] **Step 2: Run** `cargo test -p printdex-core scan` → FAIL.
- [ ] **Step 3: Implement** `db/files.rs` and `scan.rs` per the interfaces and semantics above; regenerate `.sqlx/` with `cargo sqlx prepare --workspace`.
- [ ] **Step 4: Run** `SQLX_OFFLINE=true cargo test -p printdex-core` → PASS (all tasks so far).
- [ ] **Step 5: Commit** `feat(core): incremental scan pipeline with missing-file grace`.

---

### Task 7: Search and stats

**Files:**
- Create: `crates/printdex-core/src/db/search.rs`
- Modify: `crates/printdex-core/src/db/mod.rs`, `crates/printdex-core/src/lib.rs`, `.sqlx/`

**Interfaces:**
- Consumes: `Db`, schema from Task 5.
- Produces:
  - `pub fn to_fts_query(input: &str) -> Option<String>` — `None` for blank input.
  - `#[derive(Clone, Debug, PartialEq, Eq)] pub struct ModelHit { pub id: ModelId, pub library_id: LibraryId, pub name: String, pub rel_path: String, pub kind: ModelKind, pub part_count: u32, pub missing: bool }`
  - `#[derive(Clone, Debug, Default, PartialEq, Eq)] pub struct Stats { pub libraries: u64, pub models: u64, pub parts: u64, pub assets: u64, pub missing_models: u64 }`
  - `impl Db { pub async fn search_models(&self, input: &str, limit: u32) -> Result<Vec<ModelHit>>; pub async fn stats(&self) -> Result<Stats> }`

Rules: `to_fts_query` splits on Unicode whitespace, drops empty tokens, removes `"` from each token, wraps each as a quoted prefix term `"tok"*`, and joins with a space (implicit AND). Blank input → `search_models` returns all non-missing models ordered by `name COLLATE NOCASE`, up to `limit`. Non-blank → FTS `MATCH` on `models_fts`, ranked by `bm25(models_fts, 10.0, 1.0)` (name weighted over path), missing models excluded. Reads go through the reader pool.

- [ ] **Step 1: Write failing tests**:
  - `fts_query_escapes_syntax`: `to_fts_query("dragon (v2)") == Some("\"dragon\"* \"(v2)\"*")`; `to_fts_query("say \"hi\" -x AND") == Some("\"say\"* \"hi\"* \"-x\"* \"AND\"*")`; `to_fts_query("   ") == None`.
  - `search_prefix_matches_name`: models `Red Dragon`, `Orc Warrior` → `search_models("drag", 50)` returns only `Red Dragon`.
  - `search_matches_path_segments`: model `Minis/Dragons/Red` named `Red` → `search_models("minis", 50)` finds it.
  - `search_is_diacritic_insensitive`: model `Drachen_Ü` → `search_models("drachen u", 50)` finds it.
  - `search_ranks_name_over_path`: models `Dragon` (path `Misc/Dragon`) and `Knight` (path `Dragon Pack/Knight`) → first hit for `dragon` is `Dragon`.
  - `blank_search_lists_all_sorted`: returns all non-missing models in case-insensitive name order, honoring `limit`.
  - `missing_models_excluded`: a model with `missing_since` set is not returned; `stats().missing_models == 1`.
  - `stats_counts_rows`: after inserting 1 library, 2 models, 3 parts, 1 asset → matching counts.
- [ ] **Step 2: Run** `cargo test -p printdex-core search` → FAIL.
- [ ] **Step 3: Implement**; regenerate `.sqlx/`.
- [ ] **Step 4: Run** `SQLX_OFFLINE=true cargo test -p printdex-core` → PASS.
- [ ] **Step 5: Commit** `feat(core): full-text model search and stats`.

---

### Task 8: Engine handle

**Files:**
- Create: `crates/printdex-core/src/engine.rs`, `crates/printdex-core/tests/engine.rs`
- Modify: `crates/printdex-core/src/lib.rs`

**Interfaces:**
- Consumes: everything above.
- Produces (re-exported from the crate root together with `Library`, `LibraryKind`, `ModelHit`, `Stats`, `ScanSummary`, `EngineEvent`, `ScanPhase`, `CoreError`, ids):
  - `#[derive(Clone)] pub struct Engine { db: Db, events: broadcast::Sender<EngineEvent>, scanning: Arc<Mutex<HashSet<LibraryId>>> }`
  - `impl Engine {`
    - `pub async fn open(data_dir: &Path) -> Result<Engine>` — creates `data_dir` if needed; DB at `data_dir/printdex.db`; event channel capacity 1024.
    - `pub fn default_data_dir() -> Option<PathBuf>` — `directories::ProjectDirs::from("", "", "Printdex")?.data_dir()`.
    - `pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent>`
    - `pub async fn add_library(&self, path: &Path, display_name: Option<String>) -> Result<Library>` — canonicalizes with `dunce::canonicalize` (avoids Windows `\\?\` prefixes; add `dunce` to the workspace deps); must be an existing directory with a UTF-8 path, else `LibraryPathInvalid`; rejects a path equal to, inside, or containing an existing library with `LibraryOverlaps { existing }`; default name = last path segment; `kind = Local`, `sidecar_enabled = true`; emits `LibraryAdded`.
    - `pub async fn libraries(&self) -> Result<Vec<Library>>`
    - `pub async fn library_by_path(&self, path: &Path) -> Result<Option<Library>>` — canonicalized comparison.
    - `pub async fn rescan(&self, id: LibraryId, cancel: CancellationToken) -> Result<ScanSummary>` — `AlreadyScanning` if a scan of `id` is running (guard removed on every exit path).
    - `pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<ModelHit>>`
    - `pub async fn stats(&self) -> Result<Stats>`
  - `}`

- [ ] **Step 1: Write failing integration tests** in `tests/engine.rs` (public API only):
  - `add_scan_search_roundtrip`: tree with `Red Dragon/a.stl`, `Orc/orc.stl` → `add_library`, `rescan` → `search("dragon", 10)` returns 1 hit named `Red Dragon` with `part_count == 1`.
  - `finds_models_by_non_ascii_name`: folder `龍/x.stl` and `Drachen_Ü/y.3MF` → `search("龍")` and `search("drachen")` each return 1 hit.
  - `search_with_syntax_chars_does_not_error`: for each of `"`, `*`, `(`, `-`, `AND`, `NEAR(`, `a:b` → `search(q, 10)` is `Ok`.
  - `rescan_of_offline_library_marks_nothing_missing`: scan, rename the library root away → `rescan` is `Err(LibraryOffline(_))`; `stats().missing_models == 0`; rename back, rescan → `Ok`, still 0 missing.
  - `rejects_overlapping_libraries`: add `/tmp/x/lib`; adding `/tmp/x/lib/sub` and `/tmp/x` → both `Err(LibraryOverlaps { .. })`; adding the same path again (spelled with a trailing `/.`) → `Err(LibraryOverlaps { .. })`.
  - `rejects_missing_path`: → `Err(LibraryPathInvalid(_))`.
  - `concurrent_rescan_is_rejected`: start a rescan on a library with ≥ 2,000 files without awaiting (`tokio::spawn`), immediately start a second → `Err(AlreadyScanning(_))`; first completes `Ok`; a third rescan afterwards is `Ok`.
  - `events_reach_subscribers`: `subscribe()` before `add_library` + `rescan` → receives `LibraryAdded`, `ScanStarted`, `ScanFinished`.
  - `data_persists_across_reopen`: open, add, scan, drop; reopen same `data_dir` → `libraries().len() == 1`, `search` still finds the model.
- [ ] **Step 2: Run** `cargo test -p printdex-core --test engine` → FAIL.
- [ ] **Step 3: Implement** `engine.rs`.
- [ ] **Step 4: Run** `SQLX_OFFLINE=true cargo test -p printdex-core` → PASS.
- [ ] **Step 5: Commit** `feat(core): engine handle with library management and event stream`.

---

### Task 9: CLI scan, search, stats

**Files:**
- Modify: `crates/printdex-cli/src/main.rs`, `crates/printdex-cli/Cargo.toml`
- Create: `crates/printdex-cli/tests/cli.rs`

**Interfaces:**
- Consumes: `Engine` API from Task 8.
- Produces: binary `printdex` with global option `--data-dir <PATH>` (default `Engine::default_data_dir()`), and subcommands:
  - `scan <PATH> [--name <NAME>]` — finds the library by path or adds it, rescans, prints progress to stderr (one line per phase change, using the event stream) and a final summary line to stdout: `models: +{added} missing {missing} returned {returned} · files {files_seen} · fingerprinted {fingerprinted} · skipped {skipped} · {duration_ms} ms`. Ctrl-C cancels via the `CancellationToken` and exits with code 130.
  - `search <QUERY...> [--limit <N> (default 20)]` — prints one hit per line: `{name}\t{rel_path}\t{part_count} parts` (plus `\t(missing)` if missing); exit 0 even with 0 hits.
  - `stats` — prints the `Stats` fields one per line, `key: value`.
  - Errors print `error: {CoreError display}` to stderr and exit 1. `tracing_subscriber` logs to stderr, filter from `RUST_LOG`, default `warn`.

- [ ] **Step 1: Write failing tests** in `tests/cli.rs` (run the built binary via `env!("CARGO_BIN_EXE_printdex")`, temp `--data-dir`):
  - `scan_then_search_prints_hit`: library with `Red Dragon/a.stl` → `scan` exits 0 and stdout contains `models: +1`; `search dragon` stdout contains `Red Dragon\tRed Dragon\t1 parts`.
  - `second_scan_fingerprints_nothing`: second `scan` stdout contains `fingerprinted 0`.
  - `scan_missing_path_fails`: exit code 1, stderr starts with `error:`.
  - `stats_prints_counts`: contains `models: 1` and `parts: 1`.
- [ ] **Step 2: Run** `cargo test -p printdex-cli` → FAIL.
- [ ] **Step 3: Implement** with clap derive and `#[tokio::main]`.
- [ ] **Step 4: Run** `SQLX_OFFLINE=true cargo test --workspace` → PASS.
- [ ] **Step 5: Commit** `feat(cli): scan, search and stats commands`.

---

### Task 10: Synthetic library generator, benchmark run, CI smoke

**Files:**
- Create: `crates/printdex-cli/src/bench/{mod.rs,gen.rs,run.rs}`
- Modify: `crates/printdex-cli/src/main.rs`, `crates/printdex-cli/tests/cli.rs`, `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: `Engine`.
- Produces:
  - `bench::gen::GenSpec { pub parts: u64, pub seed: u64 }`, `pub struct GenReport { pub models: u64, pub parts: u64, pub assets: u64, pub bytes: u64, pub words: Vec<String> /* name vocabulary used */ }`, `pub fn generate(root: &Path, spec: &GenSpec) -> std::io::Result<GenReport>` — deterministic for a given seed (`fastrand::Rng::with_seed`).
  - `bench::run::BenchReport { pub initial_scan_ms: u64, pub rescan_ms: u64, pub rescan_fingerprinted: u64, pub search_p50_us: u64, pub search_p95_us: u64, pub search_max_us: u64, pub queries: u32 }`, `pub async fn run(library: &Path, queries: u32, seed: u64) -> anyhow::Result<BenchReport>` — uses a fresh temp data dir.
  - CLI: `bench gen <DIR> --parts <N> [--seed <S> (default 1)]` prints the `GenReport` counts; `bench run <DIR> [--queries <N> (default 200)] [--seed <S>]` prints the `BenchReport` as `key: value` lines and exits 2 if `rescan_fingerprinted != 0`.

Generator layout (keeps disk use small: ~100k parts ≈ 1–2 GB is too much for CI, so STL bodies are tiny): 2–3 levels of category folders (`Minis/`, `Terrain/Dungeon/`, `Tools/`, …) from a fixed list; model folder names are 1–3 words from a fixed vocabulary of ~300 words including non-ASCII ones (`Drachen`, `Château`, `Ü-Boot`, `龍`); each model has 1–12 parts; 40 % of models put parts in a part folder (`STLs`, `supported`, `unsupported`); 50 % get `cover.jpg` (a few dozen bytes) and `README.md`; 5 % of parts are loose files at category level; formats 85 % `.stl`, 10 % `.3mf` (an empty zip is fine), 5 % `.obj`. STL files are valid binary STL with 1–200 triangles; some extension casing is upper-case (`.STL`). `bench run` queries are 2–5 character prefixes of words from `GenReport.words` regenerated from the seed.

- [ ] **Step 1: Write failing tests** in `tests/cli.rs`:
  - `generator_is_deterministic`: `bench gen` twice into two dirs with `--parts 500 --seed 7` → identical sorted lists of (rel_path, size).
  - `generator_hits_requested_part_count`: `--parts 500` → scanning with the engine yields exactly 500 parts in `stats()`.
  - `generated_stl_is_valid_binary_stl`: for one generated `.stl`, `len == 84 + 50 * u32::from_le_bytes(bytes[80..84])`.
  - `bench_run_reports_zero_refingerprint`: `bench run` on a 500-part library exits 0, stdout contains `rescan_fingerprinted: 0`.
- [ ] **Step 2: Run** `cargo test -p printdex-cli` → FAIL.
- [ ] **Step 3: Implement** `gen.rs`, `run.rs` and the subcommands.
- [ ] **Step 4: Run** `SQLX_OFFLINE=true cargo test --workspace` → PASS.
- [ ] **Step 5: Add the `bench-smoke` CI job** (ubuntu, release build): `cargo run --release -p printdex-cli -- bench gen $RUNNER_TEMP/lib --parts 20000 && cargo run --release -p printdex-cli -- bench run $RUNNER_TEMP/lib`, and append the output to `$GITHUB_STEP_SUMMARY`. It fails only on the non-zero exit (refingerprinting); timings are reported, not enforced, because shared CI runners are too noisy for a 50 ms gate.
- [ ] **Step 6: Run the full 100k benchmark locally once** and record the result in the PR description: `cargo run --release -p printdex-cli -- bench gen /tmp/pdx100k --parts 100000 && cargo run --release -p printdex-cli -- bench run /tmp/pdx100k`. Expected: `rescan_fingerprinted: 0`; `search_p95_us` below 50000. If p95 is above 50 ms, open a follow-up note in the PR with the numbers rather than tuning in this milestone.
- [ ] **Step 7: Commit** `feat(cli): synthetic library generator and benchmark harness`.

---

## Not in this milestone (by design)

Move/rename detection by fingerprint and the missing-file grace-period purge (milestone 4), library kind auto-detection and periodic rescans/file watching (milestone 4), scanner override rules per folder (with the UI, milestone 1+), sidecars, tags, collections, recipe and print log (milestone 3), the iced app, Fluent and the custom theme (milestone 1, after the visual design), mesh parsing and rendering (milestone 2), release pipeline and auto-updater (milestone 4), latency-injecting filesystem wrapper (milestone 4).
