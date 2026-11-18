# Printdex two-way sync: design

Status: draft for Max's review (issue #105). Post-MVP; builds on the v1 design (§6 data model, sidecars) and the backup design (targets, secrets).

## 1. Intent

Someone who uses Printdex on more than one computer should see the same tags, collections, notes, sources, recipes, print log and catalogue numbers everywhere, without thinking about it. Today v1 assumes one Printdex install per library: a second install only gets a warning and a best-effort sidecar import.

Sync is for Printdex's own data. Model files are never copied by Printdex.

## 2. Decided by Max

- **Setups (Max, 2026-11-18): both data-only setups.**
  - *Shared library*: several machines open the same library folder (NAS share, or one folder reached over the network).
  - *Own copies*: each machine has its own copy of the model files, kept in step by Dropbox, Syncthing, Nextcloud or by hand. Printdex syncs only its data.
  - Copying model files between machines is out of scope.
- **Transport (Max, 2026-11-18): a change log in a sync space** (approach A). See §4.
- Inherited: no self-hosted server (the gap Manyfold leaves), GPL-compatible deps, no `unsafe`, async (tokio) engine, model files are never modified, offline by default and network only for features the user sets up.

## 3. Goals, non-goals, anti-goals

**Goals**
- Two-way: a change made on any machine reaches every other machine; edits on two machines while offline merge without losing either record.
- Works on plain storage the user already has: a library's own folder, any folder (USB, Dropbox/Nextcloud folder), S3-compatible, WebDAV, SFTP (Linux/macOS), the same targets as backup.
- Fast and light: a sync with nothing new costs one listing; a typical edit uploads a few hundred bytes. Large libraries (100k parts) must not make sync slow.
- Safe: sync can never delete model files, and a bad or partial sync space can never wipe local data.

**Non-goals (this design)**
- Syncing model files, thumbnails or preview caches (caches are rebuilt from fingerprints).
- Syncing app settings (theme, Open with apps, backup targets, change detection). They stay per machine.
- Real-time collaboration (seconds-level live updates between users). Minutes are fine.
- Multiple people with separate accounts or permissions. A sync space is trusted by everyone who can open it.

**Anti-goals (failure even if it "works")**
- Needing a server, an account with us, or a background daemon.
- Ever moving, renaming or trashing a model file because of a sync.
- Silent data loss: a concurrent edit that disappears without a trace.
- Conflict copies or merge dialogs for everyday use.

## 4. Approaches

| Approach | Trade-offs | Verdict |
|---|---|---|
| **A. Change log in a sync space.** Each install appends small, numbered change files to its own folder in a shared spot and reads the others' folders. | One mechanism for both setups. No locking, because every file has exactly one writer (also safe in Dropbox/Syncthing folders, which make conflict copies when two machines write one file). Syncs are small and incremental. Needs compaction so a new machine doesn't replay years of changes. | **Chosen by Max.** |
| B. Periodic full exports. Each install uploads a full copy of its data and merges the others' copies. | Simplest format. Every sync moves and parses the whole library's data (megabytes at 100k models), deletes need tombstones anyway, and merging whole exports is slower. | Not recommended. |
| C. Sidecars only. Extend today's per-model `.printdex.toml` merge. | No new storage. Only covers shared libraries (own copies only if a file syncer copies sidecars too); sidecars can't hold tag/collection renames or deletes; many installs writing one sidecar race; read-only shares can't sync at all. | Not recommended. |

Using the backup repository (rustic) as transport was also considered and dropped: restic snapshots are whole-tree, encrypted packs that are made for restoring, not for merging small changes from several writers.

## 5. Concepts

- **Install**: one Printdex data folder (already has a UUID, `Engine::install_id`). Gets a user-visible **device name** (default: host name, editable), used in "changed on Laptop".
- **Sync space**: a place where change logs live. Two kinds:
  - *Library space*: `<library root>/.printdex/sync/`, created when sync is turned on for a shared library. Holds data for that library only. Needs the library root to be writable.
  - *Target space*: a sync target (folder, S3, WebDAV, SFTP) configured in Settings › Sync. Holds data for every library the user links to it. Used for own copies, and for shared libraries whose root is read-only.
- **Linked library**: a local library joined to a library in a space. All installs agree on a **library sync id** (UUID v7).
- **Sync id of a model**: the UUID all installs use for a model in a linked library. Local model ids stay as they are; a mapping table translates.

## 6. Data in the sync space

```
<space>/
  space.toml                     # format version, space id, created_at, encryption params (no secrets)
  libraries/<library-sync-id>.toml  # display name, created_by device; written once by the creating install
  installs/<install-id>/
    device.toml                  # device name, app version, last_seen (rewritten by its owner only)
    log/<seq>.chg                # change batches, seq = zero-padded u64, never rewritten
    snap/<seq>.snap              # compacted state of this install's changes up to seq
  photos/<blake3>.<ext>          # print photos, content-addressed, written once
```

Rules (all from "one writer per file"):
- An install only ever creates or replaces files under `installs/<its id>/`; `space.toml`, `libraries/*.toml` and `photos/*` are create-once and identical content wins.
- A change batch is written to a temp name and renamed (folder/SFTP/WebDAV) or put in one request (S3); readers skip names that aren't `<seq>.chg`.
- Batches are zstd-compressed, length-checked records (format details are the builder's call, e.g. postcard or JSON lines). Optional encryption: §10.

### 6.1 Change records

Each record is one field-level change with a **hybrid logical clock** stamp `(wall_ms, counter, install_id)` so ordering survives clock skew and ties break deterministically.

| Entity | Key | Fields that sync |
|---|---|---|
| Library | library sync id | display name (per install it may also stay local: builder's call, default synced) |
| Model identity | model sync id | rel path, part fingerprint set, kind (folder/file) |
| Model data | model sync id | notes, source URL, cover choice, number |
| Tag | name key | display name; membership as (model, tag) add/remove |
| Collection | collection id | name, deleted; membership (collection, model) add/remove + position |
| Recipe | model sync id | each top-level field (material, settings, required, notes, part overrides, from_attempt) |
| Attempt | attempt id | each field; deleted |
| Photo | photo id | attempt id, blob hash, file name, deleted |
| Model split/merge overrides | rel path | the scanner override ("split into models" / "merge into parent") |

Deletes are tombstone records. Set membership (tags, collections) uses add/remove records per pair; a pair is present if its newest record is an add (last-writer-wins per pair).

### 6.2 Merge rule

Per field, newest HLC stamp wins. Every install applies the same records with the same rule, so all converge regardless of order (a state-based CRDT per field). *(Claude's call: field level, not record level, so editing notes on the desktop and the recipe on the laptop never conflicts.)*

**Text that both sides edited while offline** (notes, attempt notes, recipe notes): the newer value wins as usual, and the losing value is kept in a per-model **"Edited elsewhere"** history entry with device name and time, one click to restore or copy. No dialog. *(Claude's call: meets "no silent loss" without conflict copies.)*

### 6.3 Compaction

When an install's own log passes a threshold (e.g. 2,000 records or 1 MB), it writes `snap/<seq>.snap` (the newest value of every field it owns up to seq) and deletes its older `.chg` and `.snap` files after a grace period (e.g. 30 days, so a slow reader that already saw the older files can still finish). A new install reads each install's latest snapshot plus the batches after it.

### 6.4 Read position

Each install stores, per space and per other install, the highest seq applied (`sync_cursors` table). A sync lists `installs/*/log/`, fetches only higher seqs, applies them in one DB transaction per batch, then advances the cursor. If the snapshot base is newer than the cursor (the other install compacted past us), read that snapshot.

## 7. Matching libraries and models across machines

Libraries and models already have local UUIDs, but each install made its own on its first scan. Sync must agree on shared ids.

**Library**: when sync is turned on, the user picks or creates the linked library:
- Shared library with a library space: the space lives in the library itself, so the link is automatic. The library sync id is in `.printdex/sync/space.toml`.
- Target space: Settings › Sync lists the libraries in the space; the user links a local library to one ("Models on NAS ↔ Models"), or publishes a local library as new. Printdex suggests a match by comparing part fingerprints (overlap > 50%).

**Model**: an install publishes a model identity record (rel path + part fingerprint set) the first time it syncs a model. When an install sees an identity record, it maps it to a local model by:
1. same rel path and at least one shared part fingerprint, else
2. same part fingerprint set at another path (moved or renamed folder), else
3. no match: the model's data is **parked** (kept in the DB, not shown) until a matching model appears after a scan. Settings › Sync shows "12 models from Laptop aren't on this computer". Parked data never creates folders or files.

Two installs can publish the same model before seeing each other. The rule: the identity record with the smallest sync id wins; the other becomes an alias, and its records are applied to the winner. Same for two libraries published separately and later linked.

**Catalogue numbers** (№ 0412) must stay unique per library. Numbers are synced as a model field. If two installs give the same number to different models, the model whose number record is older keeps it, the other gets the next free number on its next sync, and the change shows in "Edited elsewhere". *(Claude's call.)*

Sync also adopts the existing local data: on first link, every local record becomes a change with its current `updated_at` as the wall clock, so two machines that were used separately before merge by newest.

## 8. Sidecars and sync

Sidecars stay as they are: a mirror next to the model and a restore path. When a library is linked to a sync space:
- the "this library is also used by another Printdex" warning is replaced by the sync status;
- sidecars written by other installs that are in the same space are not imported (the log is the authority; it has field-level clocks, the sidecar doesn't);
- sidecars from installs outside the space are still imported as today.

## 9. When sync runs

- At app start, then after local changes (debounced 10 s), and every 5 minutes while the app is open. Library spaces on a watched local/removable volume also sync on watcher events for `.printdex/sync/`.
- "Sync now" in Settings › Sync and the status popover.
- Offline target or unmounted library: changes queue locally (unsent batches live in the data dir until written), status shows "Waiting for NAS".
- No background daemon: sync only runs while Printdex is open. *(Claude's call, in line with the backup scheduler.)*

## 10. Security and secrets

- Target spaces reuse the backup target code: the OpenDAL operator from `backup/remote.rs` (folder, S3, WebDAV, SFTP Linux/macOS, with the upload throttle) and the keyring-backed `backup/secrets.rs`.
- **Optional encryption with a passphrase**, same UX as backup (empty = off): a key derived from the passphrase (scrypt or argon2, params in `space.toml`) encrypts every batch, snapshot and photo with an AEAD. Crate choice is the builder's call (RustCrypto, GPL-compatible, no `unsafe` in our code). Library spaces default to off (the data already sits next to the models), target spaces default to on.
- The sync passphrase is separate from the backup passphrase.

## 11. UI

Settings › Sync:

```
Sync                                                   ◉ On
This device   [ Desktop            ]

Spaces
  ▸ Models on NAS     library space · .printdex/sync      Synced 2 min ago
  ▸ Nextcloud sync    folder · ~/Nextcloud/printdex-sync   Synced just now
      Libraries: Models ↔ Models (Laptop) · Minis ↔ Minis (Laptop)
      Devices:   Laptop (seen 3 h ago) · Desktop (this device)
      12 models from Laptop aren't on this computer   [Show]
  [+ Add space]                                         [Sync now]
```

- Per library (sidebar library menu): "Sync this library…" → choose library space (if writable) or a target space.
- Status: a small sync glyph next to the backup status in the footer; popover with last sync, queued changes, errors.
- Model view: "Edited elsewhere" entries under notes/recipe/log when a concurrent edit lost; restore/copy/dismiss.
- Attribution: attempts and notes show "on Laptop" when the last change came from another device.
- Remove a device from a space (forget its folder after it hasn't been seen for N days, with confirm); leave a space (stops syncing, keeps local data).

## 12. Engine shape

- New module `printdex-core/src/sync/`: `space.rs` (layout, read/write over the shared storage operator), `log.rs` (batch format, HLC, compaction), `merge.rs` (apply records, aliases, parking; pure and unit-tested), `identity.rs` (library/model matching), `run.rs` (job, scheduler hook, events).
- Change capture: every engine command that changes user data already goes through one place per entity (`user_data.rs`, `model_data.rs`); each also appends a record to a local `sync_outbox` table in the same transaction, so a crash can't lose a change. The sync job drains the outbox into batches.
- New tables (migration): `sync_spaces`, `sync_links` (library ↔ space, library sync id), `sync_model_map` (local model id ↔ sync id, aliases), `sync_clock` (per field stamps, or stamp columns where cheaper), `sync_outbox`, `sync_cursors`, `sync_parked`, `edit_history`.
- Events: `SyncStarted/Finished{space, applied, sent}`, `SyncFailed`, `SyncParked`.
- Engine API (sketch): `add_sync_space`, `link_library`, `unlink_library`, `sync_now`, `sync_status`, `parked_models`, `edit_history(model)`, `restore_edit`, `set_device_name`, `forget_device`.

## 13. Testing

- Property tests for `merge`: any order and any duplication of the same records converge to the same state (2 to 4 simulated installs, random edits, random delivery).
- Simulated installs over the folder backend and an in-memory operator; ChaosFs-style latency and failures mid-batch; clock skew of hours.
- Own-copies scenario: two libraries with the same fingerprints at different roots and some renamed folders; parked data appears after a scan.
- Shared-library scenario: two engines on one temp library with a library space; sidecars don't double-import.
- 100k-model library: first link, steady-state sync with nothing new (< 200 ms on folder), and a 1,000-edit batch.

## 14. Left to the builder

Batch encoding, compaction thresholds, HLC storage (separate table vs columns), AEAD/KDF crates, exact debounce and interval, the device-forget grace period, and how the "Edited elsewhere" UI looks within DESIGN.md's Frosted print language.

## 15. Open questions for Max

- None blocking. Possible later additions: syncing app settings, syncing per-library display names separately, and syncing model files (explicitly out for now).
