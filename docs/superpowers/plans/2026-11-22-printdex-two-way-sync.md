# Two-way sync Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Printdex data (tags, collections, notes, source, recipe, print log, photos, catalogue numbers) stays the same on every computer that uses a library, through per-install change logs in a sync space, with no server.

**Architecture:** A new `printdex-core/src/sync/` module. Local changes are captured by **diffing each dirty model's current state against what this install last published** (no hooks in every DB method), stamped with a hybrid logical clock, and written as one-writer-per-file batches to a *sync space* (a hidden folder in the library, or a folder/S3/WebDAV/SFTP target through the same OpenDAL services as backup). Other installs' batches are read past a cursor and merged field by field, newest stamp wins, with the losing side of a concurrent text edit kept as "Edited elsewhere". Models are matched across machines by relative path and part fingerprints; unmatched data is parked. Part B adds Settings › Sync, a status glyph, and the model view's "Edited elsewhere" and "on Laptop" hints.

**Tech Stack:** tokio, sqlx/SQLite (migration 0008), opendal 0.59 (async `Operator`), serde_json + zstd, scrypt 0.12 + chacha20poly1305 (XChaCha20-Poly1305), xxh3-128, gethostname, iced 0.14.

**Spec:** `docs/superpowers/specs/2026-11-18-printdex-sync-design.md`

## Global Constraints

- No `unsafe` code (project rule).
- Every new dependency GPL-3.0 compatible and passes `cargo deny check` (chacha20poly1305 and gethostname are MIT/Apache; scrypt 0.12 and zstd are already in the tree via rustic).
- One writer per file in a space: an install creates or replaces files only under `installs/<its install id>/`; `space.toml`, `libraries/*.toml`, `photos/*` are create-once.
- Model files are never created, moved, renamed or trashed by sync; parked data never creates folders or files.
- App settings (theme, apps, backup targets, change mode) are not synced.
- Sync runs only while Printdex is open: at start, 10 s after the last local change, every 5 minutes, and on "Sync now".
- Encryption: optional passphrase (empty = off); library spaces default off, target spaces default on; the sync passphrase is separate from the backup passphrase and lives in the same keyring store (`backup/secrets.rs`).
- SFTP only on Linux/macOS (as for backup).
- Steady-state sync with nothing new on a 100k-model library: < 200 ms on a folder space.
- PR pipeline stays under 5 minutes: new integration tests go in the single `tests/it/` binary (add a file + `mod` line) and must each run in well under a second except the 100k bench, which is `#[ignore]` and run via `printdex bench`-style manual invocation.
- sqlx: queries use `query!` macros like the rest of `db/`; regenerate `.sqlx/` with `DATABASE_URL=sqlite://$PWD/target/dev.db cargo sqlx prepare --workspace` after query changes.
- PRs target `dev`.

## Deviations from the spec (plan-level calls)

- **Change capture** is a state diff (Task 4), not an outbox row written inside every DB transaction. A persisted `sync_dirty` set (written by `Engine::changed`) plus a startup reconcile over `updated_at` columns gives the same crash safety with far less code touched.
- **Photo blobs** are addressed by xxh3-128 hex (already a dependency) instead of blake3. Content addressing only de-duplicates; a collision would at worst show the wrong photo.
- **Collections** are keyed by a collection sync id, and a remote collection maps to a local one with the same `name_key` on first sight (mirrors how sidecars join collections by name).

## Review Focus

1. **Two installs on one shared library whose sidecar writers both run**: the sidecar of an install in the same space must not be imported back as a newer change (Task 7 test `shared_library_no_sidecar_echo`).
2. **A remote batch that is truncated, half-written or from a newer format**: skipped with a `SyncFailed` warning, cursor not advanced, nothing applied (Task 2 `truncated_batch_rejected`, Task 7 `bad_batch_keeps_cursor`).
3. **Wrong or missing passphrase on an encrypted space**: sync stops with "Passphrase needed", nothing local changes (Task 2 `wrong_key_fails`, Task 7 `locked_space_reports`).
4. **Model renamed/moved on one machine**: the other machine keeps the data on the same model (by fingerprints), and no duplicate identity appears (Task 6 `moved_folder_matches_by_fingerprints`).
5. **Clock skew** (one machine an hour ahead): its later local edit doesn't lose to an earlier-by-wall-clock edit that it has already seen; HLC takes the max (Task 2 `hlc_never_goes_back`, Task 5 property test with skew).

---

## File structure

```
crates/printdex-core/
  migrations/0008_sync.sql            all sync tables
  src/sync/mod.rs                     pub types, re-exports
  src/sync/store.rs                   SpaceStore trait + FolderStore, RemoteStore, MemStore
  src/sync/crypto.rs                  passphrase → key, seal/open
  src/sync/log.rs                     Hlc, Stamp, Record, Batch encode/decode, layout paths
  src/sync/state.rs                   ModelState/Field values, diff → Records (pure)
  src/sync/merge.rs                   decide(local stamp, record) → Outcome (pure); number conflicts
  src/sync/identity.rs                model/library matching, aliases, parking (pure + db glue)
  src/sync/run.rs                     SyncState, sync_now pipeline, compaction, scheduler
  src/sync/api.rs                     Engine methods
  src/db/sync.rs                      all sync SQL
  src/backup/repo.rs                  extract `pub(crate) fn operator(...)` shared with sync
  src/user_data.rs                    Engine::changed / lists_changed also mark sync dirty
  src/sidecar.rs                      skip import from installs in the same space
  src/events.rs                       Sync* events
  tests/it/sync_*.rs                  integration tests
crates/printdex-app/src/
  sync_ui.rs                          state + update for Settings › Sync
  view/sync.rs                        Settings › Sync page, link dialog, parked list
  view/statusbar.rs                   sync glyph + popover
  view/model.rs                       Edited elsewhere, "on <device>"
  i18n/en/*.ftl                       new strings
```

---

## Part A: engine

### Task 1: Space storage

**Files:**
- Create: `crates/printdex-core/src/sync/mod.rs`, `crates/printdex-core/src/sync/store.rs`
- Modify: `crates/printdex-core/src/backup/repo.rs` (extract operator builder), `crates/printdex-core/src/lib.rs`, `Cargo.toml` (opendal feature `services-fs` not needed; folder uses std fs)
- Test: unit tests in `store.rs`; `crates/printdex-core/tests/it/sync_store.rs`

**Interfaces:**
- Produces:
  - `#[async_trait]`-free trait using boxed futures is the builder's call; signature set: `pub trait SpaceStore: Send + Sync + 'static { async fn list(&self, dir: &str) -> Result<Vec<String>>; async fn read(&self, path: &str) -> Result<Option<Vec<u8>>>; async fn write(&self, path: &str, bytes: Vec<u8>) -> Result<()>; async fn write_once(&self, path: &str, bytes: Vec<u8>) -> Result<bool>; async fn delete(&self, path: &str) -> Result<()>; fn location(&self) -> String; }` — `list` returns file names (not paths) directly in `dir`, empty when `dir` is missing; `write` is atomic (temp name `.<name>.tmp-<uuid>` + rename for folders; single put for remotes); `write_once` returns `false` without writing when the path exists.
  - `pub struct FolderStore { root: PathBuf }` (std fs on `spawn_blocking`), `pub struct RemoteStore { op: opendal::Operator }`, `pub struct MemStore` (tests; `Arc<Mutex<BTreeMap<String, Vec<u8>>>>`, plus `fail_next_writes(n)`).
  - `pub(crate) fn operator(target: &Target, secrets: &TargetSecrets, throttle: Option<Throttle>) -> Result<opendal::Operator>` in `backup/repo.rs`; `remote_backend` now wraps it.
  - `pub fn open_store(target: &Target, secrets: &TargetSecrets, throttle: Option<Throttle>) -> Result<Arc<dyn SpaceStore>>`.

- [ ] **Step 1: Write failing tests** `folder_write_is_atomic` (after `write("installs/a/log/1.chg", b"x")`, `list("installs/a/log")` == `["1.chg"]`, no `.tmp` names), `write_once_keeps_first` (second `write_once` with other bytes returns false, `read` gives first bytes), `list_missing_dir_is_empty`, `mem_store_failures` (after `fail_next_writes(1)` the write errors and nothing is stored).
- [ ] **Step 2: Run** `cargo test -p printdex-core sync::store` — FAIL (module missing).
- [ ] **Step 3: Implement** the trait and three stores; refactor `remote_backend` onto `operator`. Backup tests must still pass unchanged.
- [ ] **Step 4: Run** `cargo test -p printdex-core sync::store backup` — PASS.
- [ ] **Step 5: Commit** `feat(sync): space storage over folders and OpenDAL targets`.

### Task 2: Clock, records, batches and encryption

**Files:**
- Create: `crates/printdex-core/src/sync/log.rs`, `crates/printdex-core/src/sync/crypto.rs`
- Modify: `crates/printdex-core/Cargo.toml` (+ `chacha20poly1305 = "0.10"`, `scrypt = "0.12"` default-features off, `zstd`), workspace `Cargo.toml`
- Test: unit tests in both files

**Interfaces:**
- Produces:
  - `#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)] pub struct Stamp { pub wall_ms: i64, pub counter: u32, pub install: Uuid }` (field order gives the total order).
  - `pub struct Hlc { last: Stamp }` with `pub fn new(install: Uuid, last: Option<Stamp>) -> Hlc`, `pub fn tick(&mut self, now_ms: i64) -> Stamp` (max(now, last.wall); counter+1 when wall unchanged), `pub fn observe(&mut self, remote: Stamp)` (moves `last` up to remote's wall/counter).
  - `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Record { pub stamp: Stamp, pub key: Key, pub value: serde_json::Value }`, `pub enum Key { Library { lib: Uuid, field: LibraryField }, ModelIdentity { lib: Uuid, model: Uuid }, ModelField { lib: Uuid, model: Uuid, field: ModelField }, Tag { name_key: String }, TagMember { model: Uuid, name_key: String }, Collection { id: Uuid, field: CollectionField }, CollectionMember { id: Uuid, model: Uuid }, RecipeField { model: Uuid, field: RecipeField }, AttemptField { model: Uuid, attempt: Uuid, field: AttemptField }, Photo { attempt: Uuid, photo: Uuid }, ScanOverride { lib: Uuid, rel_path: String } }` with field enums `ModelField { Notes, Source, Number }`, `CollectionField { Name, Deleted }`, `RecipeField { Material, Settings, Required, Notes, Parts, FromAttempt, Deleted }`, `AttemptField { PrintedAt, Result, Material, Settings, DurationMin, Notes, Parts, Deleted }`. `LibraryField { Name }`. (`Key` derives `Ord, Hash, Serialize, Deserialize`; its canonical JSON is the `sync_fields.key` text.)
  - `pub struct Batch { pub format: u32 /* = 1 */, pub install: Uuid, pub seq: u64, pub observed: BTreeMap<Uuid, u64> /* highest seq of each other install the writer had applied */, pub records: Vec<Record> }`; `pub fn encode(b: &Batch, key: Option<&SpaceKey>) -> Vec<u8>`, `pub fn decode(bytes: &[u8], key: Option<&SpaceKey>) -> Result<Batch, SyncError>`. Wire: magic `b"PDXS"`, u8 version, u8 flags (bit0 encrypted), then zstd(JSON lines: header line, one record per line, trailer line `{"end":<count>}`), sealed when encrypted. Same encoding for snapshots (`Batch` with `seq` = last included seq).
  - Layout helpers: `pub fn batch_path(install: Uuid, seq: u64) -> String` = `installs/<uuid>/log/<seq:020>.chg`, `snap_path` = `installs/<uuid>/snap/<seq:020>.snap`, `device_path`, `photo_path(hash: &str, ext: &str)`, `pub fn parse_seq(name: &str, ext: &str) -> Option<u64>`.
  - `crypto.rs`: `pub struct SpaceKey([u8; 32])` (zeroized on drop not required), `pub struct KdfParams { pub salt: [u8; 16], pub log_n: u8 /* 15 */, pub r: u32 /* 8 */, pub p: u32 /* 1 */ }`, `pub fn derive(pass: &str, p: &KdfParams) -> SpaceKey` (run on `spawn_blocking`), `pub fn seal(key, plain) -> Vec<u8>` (24-byte random nonce prefix), `pub fn open(key, sealed) -> Result<Vec<u8>, SyncError>`, plus `pub fn check_value(key) -> [u8; 32]` stored in `space.toml` to tell a wrong passphrase from corruption.
  - `pub enum SyncError { Truncated, NewerFormat(u32), WrongPassphrase, PassphraseNeeded, Corrupt }` (thiserror; wrapped as `CoreError::Sync(SyncError)`).

- [ ] **Step 1: Write failing tests**: `hlc_ticks_monotonic` (three `tick(1000)` give counters 1,2,3 at wall 1000), `hlc_never_goes_back` (`tick(500)` after `tick(1000)` has wall 1000), `hlc_observe_jumps_ahead` (after `observe(Stamp{wall_ms: 5000, counter: 7, ..})`, `tick(1000)` > that stamp), `batch_roundtrip_plain`, `batch_roundtrip_encrypted`, `truncated_batch_rejected` (drop the last 10 bytes → `Err(Truncated)`; drop the trailer line before compression → `Err(Truncated)`), `newer_format_rejected` (`format: 2` → `Err(NewerFormat(2))`), `wrong_key_fails` (`Err(WrongPassphrase)`), `encrypted_without_key` (`Err(PassphraseNeeded)`), `parse_seq_ignores_tmp` (`".00000000000000000003.chg.tmp-x"` → None).
- [ ] **Step 2: Run** `cargo test -p printdex-core sync::log sync::crypto` — FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** same — PASS. `cargo deny check licenses` passes.
- [ ] **Step 5: Commit** `feat(sync): hybrid clock, change records and sealed batches`.

### Task 3: Sync tables

**Files:**
- Create: `crates/printdex-core/migrations/0008_sync.sql`, `crates/printdex-core/src/db/sync.rs`
- Modify: `crates/printdex-core/src/db/mod.rs`, `.sqlx/`
- Test: `crates/printdex-core/tests/it/sync_db.rs`

**Interfaces:**
- Produces (tables; ids BLOB UUIDs, times unix ms):
  - `sync_meta(key TEXT PK, value TEXT)` — `device_name`, `hlc` (JSON `Stamp`), `enabled`.
  - `sync_spaces(id INTEGER PK, kind TEXT /* library|target */, library_id BLOB NULL, target TEXT NULL /* backup::Target JSON */, space_id BLOB, encrypted INTEGER, kdf TEXT NULL, next_seq INTEGER, last_sync_at INTEGER NULL, last_error TEXT NULL)`.
  - `sync_links(space INTEGER, library_id BLOB, lib_sync_id BLOB, PRIMARY KEY(space, library_id))`.
  - `sync_model_map(lib_sync_id BLOB, sync_id BLOB, model_id BLOB NULL, alias_of BLOB NULL, PRIMARY KEY(lib_sync_id, sync_id))`, index on `model_id`.
  - `sync_fields(space INTEGER, key TEXT /* canonical JSON of Key */, stamp TEXT, seq INTEGER, value_hash INTEGER, value TEXT, PRIMARY KEY(space, key))` — the winning stamp and value for every key this install knows.
  - `sync_dirty(model_id BLOB PK)`, `sync_dirty_lists(name TEXT PK)` (collections/tags list changed).
  - `sync_cursors(space INTEGER, install BLOB, seq INTEGER, PRIMARY KEY(space, install))`.
  - `sync_parked(space INTEGER, lib_sync_id BLOB, sync_id BLOB, PRIMARY KEY(space, lib_sync_id, sync_id))`.
  - `sync_devices(space INTEGER, install BLOB, name TEXT, app_version TEXT, last_seen INTEGER, PRIMARY KEY(space, install))`.
  - `edit_history(id BLOB PK, model_id BLOB REFERENCES models(id) ON DELETE CASCADE, key TEXT, value TEXT, device TEXT, at INTEGER, dismissed INTEGER DEFAULT 0)`.
- `impl Db`: `mark_sync_dirty(&self, ids: &[ModelId])`, `take_sync_dirty(&self) -> Vec<ModelId>`, `mark_lists_dirty()`, `sync_fields(&self, space, keys: &[String]) -> HashMap<String, FieldRow>`, `put_sync_fields(&self, space, rows: &[FieldRow])`, `cursor(space, install) -> u64`, `set_cursor(...)`, `spaces() -> Vec<SpaceRow>`, `insert_space(&SpaceRow) -> i64`, `remove_space(id)`, `link(space, library, lib_sync_id)`, `map_model(lib_sync_id, sync_id, Option<ModelId>)`, `model_sync_id(lib_sync_id, ModelId) -> Option<Uuid>`, `local_model(lib_sync_id, sync_id) -> Option<ModelId>` (follows `alias_of`), `park`/`unpark`/`parked(space)`, `add_edit_history(...)`, `edit_history(model) -> Vec<EditEntry>`, `dismiss_edit(id)`, `upsert_device(...)`, `devices(space)`, `sync_meta_get/set`.
  - `pub struct FieldRow { pub key: String, pub stamp: Stamp, pub seq: u64, pub value_hash: u64, pub value: serde_json::Value }`.

- [ ] **Step 1: Write failing tests** `dirty_set_roundtrip` (mark 3 ids twice, take returns 3 then empty), `fields_upsert_keeps_latest_write`, `alias_resolves` (sync id B aliased to A, A mapped to model M → `local_model(B)` == M), `forget_model_cascades_history`.
- [ ] **Step 2: Run** `cargo test -p printdex-core --test it sync_db` — FAIL.
- [ ] **Step 3: Implement** migration + queries; regenerate `.sqlx/`.
- [ ] **Step 4: Run** — PASS; existing migrations tests still pass.
- [ ] **Step 5: Commit** `feat(sync): sync tables`.

### Task 4: Capturing local changes

**Files:**
- Create: `crates/printdex-core/src/sync/state.rs`
- Modify: `crates/printdex-core/src/user_data.rs` (`changed`, `lists_changed`, collection rename/delete, photo add/remove call `db.mark_sync_dirty`), `crates/printdex-core/src/scan.rs` (models created/moved by a scan mark dirty when their library is linked), `crates/printdex-core/src/file_ops.rs` (rename/move mark dirty)
- Test: unit tests in `state.rs`; `tests/it/sync_capture.rs`

**Interfaces:**
- Consumes: `Db::model_data`, `Db::organize`, `Db::model_number`, `FieldRow`, `Key`, `Hlc`.
- Produces:
  - `pub struct ModelState { pub identity: Identity, pub fields: BTreeMap<Key, serde_json::Value> }`, `pub struct Identity { pub rel_path: String, pub fingerprints: BTreeSet<u64>, pub kind: ModelKind }`.
  - `pub fn model_state(lib: Uuid, model: Uuid, data: &ModelData, number: Option<u32>, identity: Identity, collection_ids: &HashMap<String, Uuid>) -> ModelState` (pure). Tag membership keys appear with value `true`; recipe absent → `RecipeField::Deleted = true`; each attempt contributes its fields; photos contribute `Key::Photo` with `{hash, ext, file_name}`.
  - `pub fn diff(published: &HashMap<String, FieldRow>, current: &ModelState, hlc: &mut Hlc, now_ms: i64) -> Vec<Record>` (pure): one record per key whose value hash differs; keys published before but now missing become tombstones (`TagMember`/`CollectionMember` → `false`, attempt → `AttemptField::Deleted = true`, photo → `null`).
  - `pub(crate) async fn capture(db: &Db, space: i64, hlc: &mut Hlc) -> Result<Captured>` with `pub struct Captured { pub records: Vec<Record>, pub rows: Vec<FieldRow>, pub dirty: Vec<ModelId> }`: reads (does not clear) the dirty set plus, on the first call after open, every model with `data_updated_at`/recipe/attempt `updated_at` newer than `sync_meta.captured_at`; builds states for models in linked libraries and diffs. `pub(crate) async fn commit_capture(db: &Db, space: i64, c: &Captured) -> Result<()>` persists `rows`, clears exactly `dirty` and advances `captured_at`; it runs only after the batch was written, so a failed write is simply captured again next time.
  - `value_hash` = xxh3-64 of canonical JSON (`serde_json::to_vec` of a `BTreeMap`-ordered value).

- [ ] **Step 1: Write failing tests** `unchanged_state_no_records`, `notes_edit_one_record`, `removed_tag_tombstone`, `deleted_attempt_tombstone`, `recipe_removed_marks_deleted`, and integration `edit_marks_dirty_and_capture_publishes` (engine with linked library: `set_notes` then `capture` returns exactly one `ModelField{Notes}` record; second `capture` returns none), `crash_before_capture_recovered` (write notes via `Db` directly bypassing `changed`, reopen engine, capture still finds it through `updated_at`).
- [ ] **Step 2: Run** `cargo test -p printdex-core sync::state` and `--test it sync_capture` — FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(sync): capture local changes by diffing published state`.

### Task 5: Merging remote records

**Files:**
- Create: `crates/printdex-core/src/sync/merge.rs`
- Modify: `crates/printdex-core/src/db/sync.rs` (apply helpers), `crates/printdex-core/src/db/organize.rs`, `db/prints.rs` (narrow `apply_*` writers that set values without bumping `updated_at` past the remote time and without marking dirty)
- Test: unit + property tests in `merge.rs`; `tests/it/sync_merge.rs`

**Interfaces:**
- Consumes: `FieldRow`, `Record`, `Stamp`.
- Produces:
  - `pub enum Outcome { Apply, Ignore, ApplyKeepLoser { loser: serde_json::Value, loser_stamp: Stamp } }`.
  - `pub fn decide(local: Option<&FieldRow>, rec: &Record) -> Outcome` (pure): `Ignore` when `local.stamp >= rec.stamp`; `ApplyKeepLoser` when the key is a text field (`ModelField::Notes`, `AttemptField::Notes`, `RecipeField::Notes`), the local value is non-empty and differs, and the remote writer had **not seen** the local value (the local row's `seq` from its writing install is greater than `batch.observed[that install]`, or that install is missing from `observed`); else `Apply`. Signature therefore: `decide(local: Option<&FieldRow>, rec: &Record, observed: &BTreeMap<Uuid, u64>) -> Outcome`, and `FieldRow` carries `seq: u64` (the batch seq the winning stamp was published in; for this install's unsent rows, `next_seq`).
  - `pub(crate) async fn apply(db: &Db, space: i64, lib_maps: &Maps, records: Vec<Record>, device: &str) -> Result<Applied>` where `pub struct Applied { pub models: BTreeSet<ModelId>, pub lists: bool, pub parked: usize, pub history: usize }` — groups records per local model, writes through the `apply_*` DB helpers in one transaction per batch, writes `edit_history` for `ApplyKeepLoser`, updates `sync_fields`, and returns which models changed (the engine then emits `ModelDataChanged`, queues sidecars, refreshes FTS).
  - Catalogue numbers: `pub fn resolve_number(local: &FieldRow, rec: &Record, taken_by_other: bool) -> NumberOutcome { Keep, Take, Renumber }` — the older stamp keeps a contested number; the loser takes `next_number` and the change gets an `edit_history` entry.

- [ ] **Step 1: Write failing tests**: `older_record_ignored`, `newer_record_applied`, `equal_stamp_ignored`, `concurrent_notes_keep_loser`, `sequential_notes_no_history` (remote batch had observed our seq → plain Apply), `number_conflict_older_keeps`; property test `converges_any_order` (fastrand, seed loop of 200: 3 simulated installs each make 20 random edits over 5 keys with random clock skew of ±3,600,000 ms, deliver all batches to each install in a random order with random duplicates; all final `sync_fields` maps are equal and each key's value is the one with the max stamp). Integration `remote_attempt_appears` (apply a batch with an `AttemptField` set for a mapped model → `model_data` lists the attempt with the same id).
- [ ] **Step 2: Run** `cargo test -p printdex-core sync::merge` and `--test it sync_merge` — FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(sync): field-level merge with edited-elsewhere history`.

### Task 6: Matching libraries and models

**Files:**
- Create: `crates/printdex-core/src/sync/identity.rs`
- Test: unit tests; `tests/it/sync_identity.rs`

**Interfaces:**
- Consumes: `Identity`, `Db` map functions (Task 3), local part fingerprints (`db/files.rs` known files grouped by model).
- Produces:
  - `pub fn match_model(remote: &Identity, locals: &[(ModelId, Identity)]) -> Option<ModelId>` (pure): 1) same `rel_path` and ≥ 1 shared fingerprint, 2) identical fingerprint set elsewhere, 3) None.
  - `pub fn winner(a: Uuid, b: Uuid) -> Uuid` (smaller wins).
  - `pub(crate) async fn resolve_identity(db: &Db, space: i64, lib_sync_id: Uuid, sync_id: Uuid, id: &Identity) -> Result<Resolved>` with `pub enum Resolved { Mapped(ModelId), Aliased { into: Uuid }, Parked }`: if the matched local model already has another sync id, alias the larger to the smaller (and re-key its `sync_fields`).
  - `pub(crate) async fn unpark_after_scan(db: &Db, lib: LibraryId) -> Result<Vec<ModelId>>` (called after every scan of a linked library; re-runs matching for parked ids and applies their stored field rows).
  - `pub fn library_overlap(a: &BTreeSet<u64>, b: &BTreeSet<u64>) -> f32` (share of the smaller set) and `pub const SUGGEST_OVERLAP: f32 = 0.5`.

- [ ] **Step 1: Write failing tests** `same_path_matches`, `moved_folder_matches_by_fingerprints`, `different_model_same_path_no_match` (no shared fingerprint → None), `double_publish_aliases_to_smaller`, integration `parked_until_scan` (remote identity for a folder not yet on disk → `Parked`; create the folder with matching files, rescan → model has the remote notes), `overlap_suggests_library`.
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** — PASS.
- [ ] **Step 5: Commit** `feat(sync): match libraries and models across machines`.

### Task 7: Sync job, spaces and engine API

**Files:**
- Create: `crates/printdex-core/src/sync/run.rs`, `crates/printdex-core/src/sync/api.rs`
- Modify: `engine.rs` (`sync: Arc<SyncState>` field), `events.rs`, `sidecar.rs` (skip installs in the same space), `scan.rs` (call `unpark_after_scan`), `error.rs`, `printdex-cli` (`printdex sync now|status` for manual two-instance testing)
- Test: `tests/it/sync_run.rs`, `tests/it/sync_shared.rs`

**Interfaces:**
- Consumes: everything above; `backup::secrets::Store` for space passphrases and target secrets (secret ids `sync-<space id>`).
- Produces (engine API):
  - `pub async fn device_name(&self) -> Result<String>` (default `gethostname()`), `pub async fn set_device_name(&self, name: &str) -> Result<()>`.
  - `pub async fn add_library_space(&self, lib: LibraryId, passphrase: &str) -> Result<SpaceInfo>` — creates or joins `<root>/.printdex/sync/` (join when `space.toml` exists: reads its `space_id`, KDF and check value; wrong passphrase → `SyncError::WrongPassphrase`). Read-only root → `CoreError::Sync(SyncError::ReadOnly)`; the UI then offers a target space.
  - `pub async fn add_target_space(&self, target: Target, secrets: &TargetSecrets, passphrase: &str, remember: bool) -> Result<SpaceInfo>`.
  - `pub async fn space_libraries(&self, space: i64) -> Result<Vec<RemoteLibrary>>` (`{ lib_sync_id, name, created_by, suggested_local: Option<LibraryId> }`), `pub async fn link_library(&self, space: i64, lib: LibraryId, remote: Option<Uuid>) -> Result<()>` (None = publish as new; writes `libraries/<id>.toml` with `write_once`), `pub async fn unlink_library(&self, space: i64, lib: LibraryId) -> Result<()>`, `pub async fn remove_space(&self, space: i64) -> Result<()>` (keeps local data, deletes nothing remote).
  - `pub async fn sync_now(&self) -> Result<Vec<SyncReport>>`, `pub async fn sync_status(&self) -> Result<Vec<SpaceInfo>>` (`SpaceInfo { id, kind, location, encrypted, libraries, devices: Vec<Device>, last_sync_at, last_error, queued: usize, parked: usize }`), `pub async fn parked_models(&self, space: i64) -> Result<Vec<ParkedModel>>` (`{ rel_path, device }`), `pub async fn edit_history(&self, model: ModelId) -> Result<Vec<EditEntry>>`, `pub async fn restore_edit(&self, id: Uuid) -> Result<()>` (writes the kept value as a new local change), `pub async fn dismiss_edit(&self, id: Uuid) -> Result<()>`, `pub async fn forget_device(&self, space: i64, install: Uuid) -> Result<()>` (only when `last_seen` older than 30 days; deletes that install's folder in the space), `pub fn start_sync_scheduler(&self)` (start + 10 s debounce after `ModelDataChanged`/`OrganizeChanged` + every 5 min; the app calls it at boot next to `start_backup_scheduler`).
  - `pub async fn last_change_device(&self, model: ModelId) -> Result<HashMap<String, String>>` — per key, device name when the winning stamp is another install's (for "on Laptop").
  - Events: `SyncStarted { space: i64 }`, `SyncFinished { space: i64, sent: usize, applied: usize }`, `SyncFailed { space: i64, error: String }`, `SyncParked { space: i64, count: usize }`.
- Pipeline per space (`sync_once`): open store (+ key) → `capture` → if records: write `batch_path(self, next_seq)`, then `commit_capture` and advance `next_seq` (nothing is committed when the write fails) → write `device.toml` → upload new photos (`write_once`) → list `installs/` → per other install: read the newest snapshot if its seq > cursor, then batches > cursor in order; a batch that fails to decode stops that install's reading for this run with a `SyncFailed` warning and the cursor stays → `apply` → emit events → compaction when this install's batches since its last snapshot exceed 2,000 records or 1 MB (write snapshot of all `sync_fields` rows stamped by this install; delete own batches/snapshots older than 30 days that the new snapshot covers).
- Sidecar rule: `sidecar::import` skips a foreign file when its `writer` is an install in `sync_devices` for a space linked to that library; the `SidecarForeign` warning is not raised for those.

- [ ] **Step 1: Write failing tests** (two engines in temp dirs on a shared `FolderStore`): `two_installs_converge` (A tags + notes, B logs an attempt, sync A, B, A → both have all three), `offline_edits_both_survive` (A and B edit different fields of one model before syncing → both kept; same notes field → newer wins, other in `edit_history`), `bad_batch_keeps_cursor` (corrupt B's batch file → A's sync reports `SyncFailed`, cursor unchanged, A's data unchanged; fix file → next sync applies), `locked_space_reports` (encrypted target space without remembered passphrase → `SyncFailed` with `PassphraseNeeded`), `shared_library_no_sidecar_echo` (A and B on one temp library with a library space; A edits notes; after sync and a rescan on B, B's `sync_fields` stamp for notes is A's, not a new B stamp, and B publishes nothing), `photo_travels` (A attaches a photo; B gets the file in its photo dir with the same bytes), `steady_state_cheap` (second sync with no changes writes no batch), `forget_device_rules` (refuses within 30 days).
- [ ] **Step 2: Run** `cargo test -p printdex-core --test it sync_run sync_shared` — FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** — PASS; full `cargo test -p printdex-core` passes.
- [ ] **Step 5: Bench** `#[ignore]` test `sync_100k_steady` (generated 100k-part library linked to a folder space; second `sync_now` < 200 ms) — run once with `--ignored`, record the number in the PR.
- [ ] **Step 6: Commit** `feat(sync): sync job, spaces and engine API`.

## Part B: app

### Task 8: Settings › Sync

**Files:**
- Create: `crates/printdex-app/src/sync_ui.rs`, `crates/printdex-app/src/view/sync.rs`
- Modify: `app.rs` (`SettingsSection::Sync` between Backup and About, messages), `view/settings.rs`, `main.rs`/boot (`start_sync_scheduler`), `i18n/en/*.ftl`, `icons.rs` (sync icon), reuse the backup target form from `view/backup.rs` (extract `target_form` into a shared widget fn if it isn't one already)
- Test: unit tests on `sync_ui` state transitions

**Interfaces:**
- Consumes: Task 7 engine API.
- Produces: page per spec §11: on/off, device name field, space list (kind, location, last sync, libraries ↔, devices with last seen, parked count + Show), "Add space" (choose "Inside a library" → library picker, or "Another place" → target form + passphrase, defaults: encryption off for library space, on for target), link dialog listing `space_libraries` with the suggested match preselected and "New in this space", "Sync now", remove space (confirm), forget device (enabled only after 30 days).

- [ ] **Step 1: Write failing tests** `add_space_flow_defaults` (library space → encrypted false; target → true), `link_dialog_preselects_suggestion`, `forget_disabled_recent_device`.
- [ ] **Step 2: Run** `cargo test -p printdex-app sync_ui` — FAIL.
- [ ] **Step 3: Implement** in the Frosted print language (DESIGN.md); strings in Fluent.
- [ ] **Step 4: Run** — PASS. Run the app under Xvfb with two data dirs on a shared folder space and screenshot the page (dark + light).
- [ ] **Step 5: Commit** `feat(app): Settings › Sync`.

### Task 9: Status, Edited elsewhere and attribution

**Files:**
- Modify: `view/statusbar.rs` (sync glyph next to backup status; popover: last sync, queued, errors, Sync now), `view/model.rs` (Edited elsewhere list under notes/recipe/log with Restore / Copy / Dismiss; "on Laptop" on attempts and notes), `app.rs` (event handling for `Sync*`, refresh model on `ModelDataChanged` from sync), sidebar library menu "Sync this library…" opening Settings › Sync with that library preselected, i18n.
- Test: unit tests for the status text helper

**Interfaces:**
- Consumes: `sync_status`, `edit_history`, `restore_edit`, `dismiss_edit`, `last_change_device`, `Sync*` events.
- Produces: `pub fn sync_status_text(info: &[SpaceInfo], now_ms: i64) -> String` ("Synced 2 min ago", "Waiting for NAS", "Passphrase needed", "Sync off").

- [ ] **Step 1: Write failing tests** `status_text_cases` (each of the four strings above from crafted `SpaceInfo`s).
- [ ] **Step 2: Run** — FAIL.
- [ ] **Step 3: Implement**.
- [ ] **Step 4: Run** — PASS. Screenshots: status popover, a model with an Edited elsewhere entry, an attempt "on Laptop".
- [ ] **Step 5: Commit** `feat(app): sync status, edited elsewhere and device attribution`.

---

## Delivery

- Part A (Tasks 1–7) = one PR into `dev`; Part B (Tasks 8–9) = a second PR with screenshots.
- Issues: one per task under a "Two-way sync" epic (design issue #105 closes with the epic).
