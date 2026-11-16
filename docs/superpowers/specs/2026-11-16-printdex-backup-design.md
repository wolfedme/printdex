# Printdex Backup & Restore — Design

*Status: approved by Max · 2026-11-16*

Extends the [v1 design](2026-11-16-printdex-v1-design.md) and the [architecture design](2026-11-16-printdex-architecture-design.md). Where this document disagrees with them, this one wins.

## 1. Intent

**Why (Max):** users must not lose a library. A dead disk, a wiped laptop or an accidental delete should never cost someone their models or, worse, the tags, recipes and print log they built up over months, which exist nowhere else.

So the MVP gets **backup and restore**: Printdex copies its own data, and optionally the model files, to a storage target the user chooses, keeps restorable points in time, and can rebuild the library from them on the same or a new machine.

This is disaster recovery, not multi-machine use. Two-way sync between machines stays post-MVP.

## 2. Goals, non-goals, anti-goals

**Goals**
- Back up Printdex data (always) and model files (opt-in per library) to a folder or a remote server of the user's choice.
- Keep history as snapshots, while keeping storage use low: store-once deduplication, compression, automatic thinning of old snapshots.
- Optional encryption with the user's own passphrase.
- Restore everything onto a fresh install, or restore a single model.
- Run unattended in the background; never get in the way of browsing.
- Make backup health visible: the user always knows when the last good backup was.

**Non-goals (MVP)**
- Two-way sync / using one library from several machines at once (post-MVP).
- Backing up arbitrary non-library files.
- Hosted Printdex cloud storage or accounts. The user brings their own target.
- Browsing backups as plain files (backups are in the restic format; see §4).

**Anti-goals**
- Silent backup failure. A backup that stopped working weeks ago without the user knowing is the worst outcome.
- A backup that only Printdex can ever read.
- Backup work making the app feel slow (scrolling, search, previews).
- Uploading anything without the user having set up a target.

## 3. Changes to the v1 and architecture designs

- **"Never uploads" promise dropped (Max).** v1 §1 "never uploads anything" and the anti-goal "any cloud upload of user files…" are replaced by: *"Printdex only uploads to storage the user configured. Using the app never requires an account, an upload or telemetry."* The project tagline should be updated to match.
- **Constraint "Fully offline"** becomes: offline by default; network only for features the user explicitly sets up (backup targets, update check, later downloads/sync).
- **v1 §2 Post-MVP** keeps "Sync of Printdex data between machines"; backup moves into v1 goals.
- **Milestones** *(Claude's call)*: add **milestone 5 — Backup & restore**, after milestone 4. Reasons: backup's most valuable content (recipes, print log) only exists from milestone 3, and NAS hardening in milestone 4 settles library identity and offline handling that backup relies on. MVP is not released without milestone 5.
- **Architecture stack table**: add `rustic_core` + `rustic_backend` (backup engine and storage backends) and `keyring` (OS credential store).

## 4. Approaches considered

**How backups reach their destination** (decided by Max: B)

| Option | Trade-off | Verdict |
|---|---|---|
| A. Any folder only | Smallest; cloud only via the provider's desktop client. | Not chosen. |
| **B. Any folder + built-in remotes** (S3-compatible, WebDAV, SFTP) | No extra software for users; more work (credentials, resumable uploads, slow links). | **Chosen by Max.** |
| C. Drive external rclone/restic | Many backends for free; users must install a tool; breaks when it changes. | Not chosen. |

**Backup engine** (decided by Max: A)

| Option | Trade-off | Verdict |
|---|---|---|
| **A. rustic_core** (Rust library writing the restic repository format) | Dedup, zstd compression, encryption, snapshots, retention and backends are built in and battle-tested; backups restorable with standard `restic`/`rustic` tools even without Printdex. Large dependency, synchronous API, release pace not ours; "encryption off" means an empty passphrase (data still scrambled, but anyone holding the backup can open it). | **Chosen by Max.** |
| B. Own format (content-addressed blobs + zstd + age + snapshot manifests) | Exact fit, truly plain storage when unencrypted. Much more code in the place where bugs silently lose data; only Printdex can read it. | Not chosen. |

**What a backup includes by default** (decided by Max: data always, models opt-in per library). Alternatives were "everything" (big first upload, often duplicates a NAS that's already backed up) and "data only" (not a full restore).

**History** (decided by Max): snapshots, not a mirror, because a mirror copies deletions and corruption along with everything else.

## 5. What gets backed up

**Printdex data snapshot** (always, once a target is set up):
- A consistent copy of the SQLite database, taken with SQLite's online backup / `VACUUM INTO` into a temp file, never by copying the live file.
- Attempt photos and any other user-authored files in the app-data dir.
- App settings (libraries, their paths and kind, backup config **without** secrets).
- Not included: thumbnail/preview caches (regenerable), logs, credentials.

**Model files snapshot** (per library, opt-in):
- Every file under the library root that the scanner treats as part of a model (parts, assets, sidecars). *(Claude's call)* Use the same ignore rules as the scanner, so backup and library agree on what exists.
- Default: **on** for libraries of kind local/removable, **off** for network libraries (they usually sit on a NAS with its own backup). This is the default Max picked on the decision card ("opt-in" in the sense that it's a per-library switch).
- The first model backup never starts silently: when a target is set up (or a library is added), the setup shows each library's estimated size and the model switches pre-set to the defaults above, and the user confirms. *(Claude's call)*

Snapshots are tagged so they can be told apart: `printdex-data` and `printdex-models:<library uuid>`, with the install's id as the restic host name. Thinning groups by **host + tag**, so two installs sharing a repository never thin each other's snapshots. *(Claude's call)*

**Install identity after a restore** *(Claude's call)*: a restored install gets a **new** install id (the old machine may still be alive). It stores the restored-from install id as a *predecessor*: sidecars written by a predecessor are treated as its own (no "another Printdex" warning). Snapshots of other hosts stay listed in backup settings, where the user can "Forget this computer's backups" to let them be pruned; otherwise they're kept as-is.

## 6. Targets

- **Folder**: any local or mounted path (USB drive, NAS share, Dropbox/Nextcloud/OneDrive synced folder).
- **S3-compatible**: endpoint, bucket, region, access key, secret (AWS S3, Backblaze B2, Wasabi, MinIO, Cloudflare R2, …).
- **WebDAV**: URL, user, password (Nextcloud, ownCloud, many NAS boxes).
- **SFTP** (Linux and macOS only, decided by Max): host, port, user, password or key file.

All via `rustic_backend` (folder backend + OpenDAL-based remotes). MVP supports **one target** per install. *(Claude's call: multiple targets, e.g. USB plus cloud, is a natural follow-up; the config should be a list from day one so it doesn't need migration.)*

Secrets (remote passwords/keys, and the passphrase if the user lets Printdex remember it) live in the OS credential store via `keyring` (Keychain, Windows Credential Manager, Secret Service). Never in the DB, settings file or a backup. If no credential store is available (some minimal Linux setups), Printdex asks for the secret when a backup runs and says why. *(Claude's call)*

"Test connection" in the setup form checks reachability and write access before saving.

## 7. Encryption (decided by Max: optional, own passphrase)

- At setup: "Encrypt backups" toggle with a passphrase field (entered twice). *(Claude's call)* Default **on** for remote targets and **off** for folder targets, the user can change either.
- Off = repository created with an empty passphrase (restic format always encrypts; this is the accepted consequence of engine A).
- "Remember passphrase on this computer" (default on) stores it in the OS credential store so backups run unattended. If off, scheduled backups pause and the status indicator asks for the passphrase.
- Clear warning at setup: "If you lose this passphrase, nobody can restore this backup, including Printdex." Require a checkbox acknowledgment.
- Changing the passphrase later is supported (restic key management) without re-uploading.
- Turning encryption on/off for an existing backup is **not** supported in MVP; it means starting a new backup at a new location. *(Claude's call)*

## 8. Keeping space low (Max: low space use is a priority; compression wanted)

- **Store once**: restic content-defined chunking deduplicates across files, folders, libraries and snapshots. A model present in 100 snapshots is stored once.
- **Compression**: repository format v2 with zstd, on by default (default level). STL/OBJ compress well; already-zipped 3MF gains little.
- **Snapshot thinning** *(Claude's call, user-adjustable)*: keep the last 7 daily, 4 weekly and 12 monthly snapshots per tag; older ones are forgotten. A deleted model stays restorable until its last snapshot ages out (about a year with defaults).
- **Prune** (actually freeing space) runs after forgetting, at most weekly, in the background. *(Claude's call)*
- **Space shown** in Backup settings: total repository size and per-library model size, plus the estimate before enabling models for a library.

## 9. When backups run

- **Printdex data**: automatically, debounced, a few minutes after the last change *(Claude's call: 5 min idle)*, and at least once a day. It's small and fast.
- **Model files**: on a schedule *(default daily; options: daily, weekly, manual only)*, plus whenever a scan detected changes in a library and the last model backup is older than the schedule.
- **"Back up now"** button runs both immediately.
- Backups run while the app is open. *(Claude's call)* No background service or OS scheduler in MVP. If the app wasn't open when a backup was due, it runs shortly after the next launch.
- Model backups skip libraries that are offline; they don't fail the whole run.
- Interrupted uploads resume: restic only uploads packs the repository doesn't already have, so a re-run continues where it stopped.
- Backup work runs at low priority: limited worker threads, and it yields to scanning, thumbnails and previews. Optional upload bandwidth limit in settings. *(Claude's call)*
- Closing the app during a backup: finish the current data snapshot if it's nearly done, otherwise cancel cleanly; the next run resumes. Never block quitting for more than a couple of seconds.

## 10. Restore

**Full restore (new or wiped machine)**
1. First launch shows "Start fresh" or "Restore from backup". Also available in Settings → Backup.
2. User enters the target and passphrase (if any), and picks a point in time (newest by default) from the list of `printdex-data` snapshots, shown by date and host.
3. Printdex restores the DB, photos and settings into a staging dir, then swaps them in and restarts.
4. For each library, Printdex asks where it lives now:
   - **"It's already here"**: point at an existing folder (e.g. the NAS share is still fine). Printdex re-links and rescans; content fingerprints reconnect models that moved.
   - **"Restore files from backup"** (only if that library has model snapshots): choose a destination folder; files are restored there, then scanned.
   - **"Skip for now"**: library shows as offline, data intact.
5. Library roots are re-pointed; all model paths in the DB are already relative to the library root, so nothing else changes.
6. **The restored point in time wins over sidecars** *(Claude's call)*: the user may deliberately restore an older state, so the post-restore scan does not import sidecar data that is newer than the restored DB. Instead Printdex rewrites the sidecars from the restored DB. Model files restored from backup bring their sidecars with them, which match anyway.

**Single model restore**: Model view → "Restore from backup…" lists snapshots containing that model folder, previews which files differ, and restores into the original folder (existing files are moved to the OS trash first, never overwritten in place) or into a chosen folder. *(Claude's call)*

**Recovering a deleted model**: the "missing" models list (v1 §7) offers "Restore from backup" for models whose files are gone.

Single-model restore restores that model's `.printdex.toml` too and imports it, so the model's tags, recipe and log return to the chosen point in time as well. *(Claude's call)*

Restores never delete or overwrite user files silently, consistent with v1 §8.

## 11. Status and failure handling

- Status indicator (sidebar footer or title bar): idle / backing up (with progress) / last successful backup time / needs attention.
- **Needs attention** when: the last successful data backup is older than 3 days *(Claude's call)*, a library with model backup on has had no successful model backup for 14 days *(Claude's call; for removable/network libraries that were offline the message is "Connect <library> so it can be backed up")*, the target is unreachable or full, credentials are rejected, the passphrase is needed, or an integrity check failed. Clicking it explains what's wrong and how to fix it.
- Transient errors (network, target offline) retry with backoff; they only become "needs attention" if they persist past the threshold.
- **Integrity check** *(Claude's call)*: monthly, verify repository structure and read back a small random sample of data (rustic `check` with a read subset) so silent corruption is noticed.
- Backup history view: last ~50 runs with result, duration, data added.
- One install writes to a repository at a time; restic locks handle concurrent access. Two installs pointed at the same repository is allowed (different host names) but their snapshots are separate; Printdex shows the warning from v1 §6 when restoring data from another host.

## 12. Architecture fit

- New module `backup` in `printdex-core`, behind the same `Engine` handle: commands (`configure_backup`, `backup_now`, `restore_data`, `restore_model`, `list_snapshots`, `check_backup`), events (`BackupStarted/Progress/Finished/Failed`, `BackupNeedsAttention`).
- `rustic_core` is synchronous: each backup/restore job runs on a dedicated blocking thread (`spawn_blocking` or a small dedicated pool), reporting progress back through a channel into the event stream, and honouring a cancellation token between files/packs.
- A backup scheduler task in the engine owns timers, debounce and the "due on launch" logic.
- The DB snapshot uses sqlx/SQLite's backup into a temp file; the live DB keeps serving reads and writes.

## 13. Verified before planning (2026-11-16)

- **License: OK.** `rustic_core` 0.13 and `rustic_backend` are `Apache-2.0 OR MIT`. Their full dependency tree (~430 crates, checked for the Windows target) is MIT/Apache-2.0/BSD/ISC/Zlib/MPL-2.0/Unicode-3.0/CC0/Unlicense, all GPL-3.0-compatible. `cargo-deny` stays the enforcement in CI.
- **SFTP: not available on Windows** through `rustic_backend` (OpenDAL's SFTP service is Unix-only). **Decided by Max:** no custom SFTP backend; SFTP is offered on **Linux and macOS only**. On Windows the SFTP option is not shown in the target list (folder, S3 and WebDAV remain).
- **Empty passphrase:** `rustic_core` takes the password as a plain string and has no check rejecting an empty one; confirm with an init/backup/restore round-trip test in the first milestone-5 task.
- **Weight:** rustic pulls in a sizeable dependency tree (~430 crates, OpenDAL, reqwest, its own rayon/tokio use). Measure binary size and build time in the first milestone-5 task; trim OpenDAL service features to only folder/S3/WebDAV.

## 14. Decided by Max

- Backup & restore is in the MVP; reason: not losing a library. Two-way sync stays post-MVP.
- "Never uploads" promise dropped.
- Targets: any folder + built-in remotes.
- Contents: Printdex data always, model files opt-in per library.
- History: snapshots; keep space low; compression.
- Encryption: optional, user's own passphrase, for general privacy.
- Engine: rustic_core (restic format).
- SFTP on Linux and macOS only; no custom SFTP backend for Windows.

## 15. Claude's calls (check while reading)

- Milestone 5 placement, after NAS hardening.
- Model files default on for local/removable, off for network libraries.
- Encryption default on for remote targets, off for folders; remember passphrase in OS credential store by default.
- One target in MVP (config shaped as a list).
- Retention 7 daily / 4 weekly / 12 monthly; weekly prune; monthly sample integrity check.
- Data backup 5 min after last change and at least daily; model backup daily; only while the app is open.
- "Needs attention" after 3 days without a successful data backup, or 14 days without a model backup for a library that has it on.
- Restored installs get a new install id with the old one recorded as predecessor; thinning by host + tag.
- After a restore, the restored data wins over newer sidecars, and sidecars are rewritten.
- First model upload needs a confirm step showing sizes.
- Single-model restore trashes existing files rather than overwriting.

## 16. Left to the builder

- Chunker parameters, zstd level, pack sizes (rustic defaults unless benchmarks say otherwise).
- Exact UI of the backup settings, history and restore flows (follows the visual design work).
- Retry/backoff timings, thread counts, bandwidth limiter implementation.
