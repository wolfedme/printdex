//! One sync of one space (plan Task 7): capture local changes and write
//! them as a batch, read every other install's new batches, merge them,
//! bring local models up to date, and compact this install's log.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::crypto::{self, KdfParams, SpaceKey};
use super::identity::LocalIndex;
use super::log::{
    self, AttemptField, Batch, CollectionField, FORMAT, Hlc, Key, ModelField, RecipeField, Record,
    Stamp,
};
use super::merge::{self, NumberOutcome, Outcome};
use super::state::{self, Identity};
use super::store::SpaceStore;
use crate::db::{Db, DeviceRow, FieldRow, Organize, SpaceRow};
use crate::engine::Engine;
use crate::error::{CoreError, Result, SyncError};
use crate::events::EngineEvent;
use crate::ids::{AttemptId, CollectionId, LibraryId, ModelId, PhotoId};
use crate::model_data::{Attempt, AttemptResult, PrintSettings, Recipe};
use crate::time::now_ms;

/// Most records in one batch file.
pub const BATCH_RECORDS: usize = 5_000;
/// Records written since the last snapshot after which this install
/// compacts its log.
pub const SNAPSHOT_AFTER: u64 = 2_000;
/// How often `device.toml` is rewritten when nothing else changes.
const DEVICE_REFRESH_MS: i64 = 6 * 60 * 60 * 1000;

/// `space.toml`: what every install needs to open the space.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpaceFile {
    pub format: u32,
    pub space_id: Uuid,
    pub created_at: i64,
    /// The one library of a library space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<KdfParams>,
    /// Sealed check value (encrypted spaces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
}

/// `libraries/<id>.toml`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LibraryFile {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub created_by: String,
}

/// `installs/<id>/device.toml`
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceFile {
    pub name: String,
    #[serde(default)]
    pub app_version: String,
    pub last_seen: i64,
}

pub const SPACE_FILE: &str = "space.toml";

pub fn library_path(id: Uuid) -> String {
    format!("libraries/{}.toml", id.hyphenated())
}

/// Shared sync state of an engine.
#[derive(Debug, Default)]
pub struct SyncState {
    /// One sync at a time.
    pub(crate) running: tokio::sync::Mutex<()>,
    /// Derived keys of encrypted spaces, by space row id.
    pub(crate) keys: Mutex<HashMap<i64, SpaceKey>>,
    /// Models changed in this session, not yet persisted as dirty.
    pub(crate) dirty: Mutex<HashSet<ModelId>>,
    /// When device.toml was last written, per space.
    pub(crate) device_written: Mutex<HashMap<i64, i64>>,
    /// Spaces whose libraries need a matching pass (first sync, links,
    /// scans).
    pub(crate) rematch: Mutex<HashSet<i64>>,
    /// Whether this session did its first-capture reconcile.
    pub(crate) reconciled: std::sync::atomic::AtomicBool,
    /// Wakes the scheduler.
    pub(crate) poke: tokio::sync::Notify,
    pub(crate) scheduler: std::sync::atomic::AtomicBool,
    /// Compaction threshold override (tests); 0 = [`SNAPSHOT_AFTER`].
    pub(crate) snapshot_after: std::sync::atomic::AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl SyncState {
    pub(crate) fn mark(&self, id: ModelId) {
        lock(&self.dirty).insert(id);
        self.poke.notify_one();
    }

    pub(crate) fn rematch_all(&self, spaces: impl IntoIterator<Item = i64>) {
        lock(&self.rematch).extend(spaces);
    }
}

/// What one space's sync did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub space: i64,
    pub sent: usize,
    pub applied: usize,
    pub parked: usize,
    /// Kept losing edits.
    pub history: usize,
    /// A problem that didn't stop the sync (an unreadable batch).
    pub warning: Option<String>,
}

/// Secret store keys.
pub(crate) fn pass_key(space: i64) -> String {
    format!("sync-{space}-passphrase")
}

pub(crate) fn secret_key(space: i64) -> String {
    format!("sync-{space}-secret")
}

/// A space opened for one sync.
pub(crate) struct Open {
    pub row: SpaceRow,
    pub store: Arc<dyn SpaceStore>,
    pub key: Option<SpaceKey>,
    pub me: Uuid,
    pub device: String,
}

impl Engine {
    /// This install's name in sync spaces (the host name until changed).
    pub async fn device_name(&self) -> Result<String> {
        if let Some(n) = self.db.sync_meta("device_name").await? {
            return Ok(n);
        }
        Ok(gethostname::gethostname()
            .to_str()
            .map(str::to_owned)
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| "Printdex".into()))
    }

    /// Renames this install in sync spaces.
    pub async fn set_device_name(&self, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err(CoreError::NameInvalid);
        }
        self.db.set_sync_meta("device_name", name).await?;
        lock(&self.sync.device_written).clear();
        Ok(())
    }

    /// Whether sync runs (Settings › Sync "On"). On until turned off.
    pub async fn sync_enabled(&self) -> Result<bool> {
        Ok(self.db.sync_meta("enabled").await?.as_deref() != Some("0"))
    }

    /// Turns sync on or off. Off keeps the spaces and the local data;
    /// changes made meanwhile go out when it is turned on again.
    pub async fn set_sync_enabled(&self, on: bool) -> Result<()> {
        self.db
            .set_sync_meta("enabled", if on { "1" } else { "0" })
            .await?;
        if on {
            self.sync.poke.notify_one();
        }
        Ok(())
    }

    /// How many changed models wait for the next sync.
    pub async fn sync_queued(&self) -> Result<usize> {
        let mut ids: HashSet<ModelId> = lock(&self.sync.dirty).clone();
        ids.extend(self.db.sync_dirty().await?);
        Ok(ids.len())
    }

    /// Compacts after `n` records instead of [`SNAPSHOT_AFTER`] (tests).
    #[doc(hidden)]
    pub fn set_sync_snapshot_after(&self, n: u64) {
        self.sync
            .snapshot_after
            .store(n, std::sync::atomic::Ordering::Relaxed);
    }

    pub(crate) fn mark_sync_dirty(&self, id: ModelId) {
        self.sync.mark(id);
        let db = self.db.clone();
        tokio::spawn(async move {
            if let Err(e) = db.mark_sync_dirty(&[id]).await {
                tracing::warn!(error = %e, "could not remember a change for sync");
            }
        });
    }

    async fn hlc(&self) -> Result<Hlc> {
        let last = self
            .db
            .sync_meta("hlc")
            .await?
            .and_then(|s| serde_json::from_str::<Stamp>(&s).ok());
        Ok(Hlc::new(self.install, last))
    }

    async fn save_hlc(&self, hlc: &Hlc) -> Result<()> {
        self.db
            .set_sync_meta(
                "hlc",
                &serde_json::to_string(&hlc.last()).unwrap_or_default(),
            )
            .await
    }

    /// The store of a space row.
    pub(crate) async fn space_store(&self, row: &SpaceRow) -> Result<Arc<dyn SpaceStore>> {
        match row.kind {
            crate::db::SpaceKind::Library => {
                let lib = row.library_id.ok_or(SyncError::SpaceNotFound)?;
                let lib = self.db.library(lib).await?;
                Ok(Arc::new(super::store::FolderStore::new(library_space_dir(
                    &lib.path,
                ))))
            }
            crate::db::SpaceKind::Target => {
                let target: crate::backup::Target =
                    serde_json::from_str(row.target.as_deref().unwrap_or("null"))
                        .map_err(|_| SyncError::SpaceNotFound)?;
                let secrets = crate::backup::TargetSecrets {
                    secret: self.secrets.get(&secret_key(row.id)),
                    passphrase: String::new(),
                };
                let limit = self.backup_config().await?.schedule.upload_limit_kib;
                super::store::open_store(&target, &secrets, limit)
            }
        }
    }

    /// The key of an encrypted space, from the cache or the stored
    /// passphrase.
    pub(crate) async fn space_key(
        &self,
        row: &SpaceRow,
        file: &SpaceFile,
    ) -> Result<Option<SpaceKey>> {
        if !row.encrypted && file.kdf.is_none() {
            return Ok(None);
        }
        if let Some(k) = lock(&self.sync.keys).get(&row.id) {
            return Ok(Some(k.clone()));
        }
        let pass = self
            .secrets
            .get(&pass_key(row.id))
            .ok_or(SyncError::PassphraseNeeded)?;
        let key = derive_checked(&pass, file).await?;
        lock(&self.sync.keys).insert(row.id, key.clone());
        Ok(Some(key))
    }

    pub(crate) async fn open_space(&self, row: SpaceRow) -> Result<Open> {
        let store = self.space_store(&row).await?;
        let bytes = store
            .read(SPACE_FILE)
            .await?
            .ok_or(SyncError::Unreachable("space.toml is missing".into()))?;
        let file = parse_space_file(&bytes)?;
        if file.space_id != row.space_id {
            return Err(SyncError::SpaceNotFound.into());
        }
        let key = self.space_key(&row, &file).await?;
        Ok(Open {
            row,
            store,
            key,
            me: self.install,
            device: self.device_name().await?,
        })
    }

    /// Syncs every space now. Spaces that fail report through
    /// [`EngineEvent::SyncFailed`] and their `last_error`.
    pub async fn sync_now(&self) -> Result<Vec<SyncReport>> {
        let _guard = self.sync.running.lock().await;
        if !self.sync_enabled().await? {
            return Ok(Vec::new());
        }
        let spaces = self.db.sync_spaces().await?;
        if spaces.is_empty() {
            return Ok(Vec::new());
        }
        // Changes of this session plus those an earlier one left.
        let mut dirty: BTreeSet<ModelId> = std::mem::take(&mut *lock(&self.sync.dirty))
            .into_iter()
            .collect();
        dirty.extend(self.db.sync_dirty().await?);
        if !self
            .sync
            .reconciled
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            // A crash can lose the dirty marks of the last seconds; the
            // change times still tell.
            let since: i64 = self
                .db
                .sync_meta("captured_at")
                .await?
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            for s in &spaces {
                for (lib, _) in self.db.sync_links(s.id).await? {
                    dirty.extend(self.db.models_changed_since(lib, since).await?);
                }
            }
        }
        let started = now_ms();
        let mut hlc = self.hlc().await?;
        let mut reports = Vec::new();
        let mut all_ok = true;
        for row in spaces {
            let id = row.id;
            let _ = self.events.send(EngineEvent::SyncStarted { space: id });
            let res = match self.open_space(row).await {
                Ok(open) => self.sync_space(&open, &dirty, &mut hlc).await,
                Err(e) => Err(e),
            };
            match res {
                Ok(r) => {
                    self.db
                        .set_space_result(id, Some(now_ms()), r.warning.as_deref())
                        .await?;
                    let _ = self.events.send(EngineEvent::SyncFinished {
                        space: id,
                        sent: r.sent,
                        applied: r.applied,
                    });
                    if r.parked > 0 {
                        let _ = self.events.send(EngineEvent::SyncParked {
                            space: id,
                            count: r.parked,
                        });
                    }
                    reports.push(r);
                }
                Err(e) => {
                    all_ok = false;
                    tracing::warn!(space = id, error = %e, "sync failed");
                    self.db
                        .set_space_result(id, None, Some(&e.to_string()))
                        .await?;
                    let _ = self.events.send(EngineEvent::SyncFailed {
                        space: id,
                        error: Arc::new(crate::scan::clone_error(&e)),
                    });
                }
            }
        }
        self.save_hlc(&hlc).await?;
        if all_ok {
            let ids: Vec<ModelId> = dirty.into_iter().collect();
            self.db.clear_sync_dirty(&ids).await?;
            self.db
                .set_sync_meta("captured_at", &started.to_string())
                .await?;
        } else {
            let ids: Vec<ModelId> = dirty.into_iter().collect();
            self.db.mark_sync_dirty(&ids).await?;
        }
        Ok(reports)
    }

    async fn sync_space(
        &self,
        open: &Open,
        dirty: &BTreeSet<ModelId>,
        hlc: &mut Hlc,
    ) -> Result<SyncReport> {
        let space = open.row.id;
        let mut report = SyncReport {
            space,
            ..SyncReport::default()
        };
        // 1. Publish.
        let captured = self.capture(open, dirty, hlc).await?;
        report.sent = captured.records.len();
        self.publish(open, captured).await?;
        self.write_device(open).await?;
        // 2. Read the others.
        let mut touched = Touched::default();
        let installs = open.store.list_dirs("installs").await?;
        for name in installs {
            let Ok(other) = Uuid::parse_str(&name) else {
                continue;
            };
            if other == open.me {
                continue;
            }
            match self.read_install(open, other, hlc, &mut touched).await {
                Ok(n) => report.applied += n,
                Err(
                    e @ CoreError::Sync(
                        SyncError::Truncated | SyncError::Corrupt | SyncError::NewerFormat(_),
                    ),
                ) => {
                    // That install's next batch is unreadable: stop reading
                    // it for now, keep the cursor, report it.
                    tracing::warn!(install = %other, error = %e, "skipping an unreadable sync batch");
                    let _ = self.events.send(EngineEvent::SyncFailed {
                        space,
                        error: Arc::new(crate::scan::clone_error(&e)),
                    });
                    report.warning = Some(e.to_string());
                }
                Err(e) => return Err(e),
            }
        }
        // 3. Bring local data up to date.
        report.history = touched.losers.len();
        self.apply_touched(open, touched).await?;
        report.parked = self.db.parked(space).await?.len();
        // 4. Compact.
        let row = self
            .db
            .sync_space(space)
            .await?
            .ok_or(SyncError::SpaceNotFound)?;
        let threshold = match self
            .sync
            .snapshot_after
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            0 => SNAPSHOT_AFTER,
            n => n,
        };
        if row.since_snap >= threshold {
            self.compact(open, &row).await?;
        }
        Ok(report)
    }

    // ---------------------------------------------------------------
    // Capture and publish
    // ---------------------------------------------------------------

    pub(crate) async fn capture(
        &self,
        open: &Open,
        dirty: &BTreeSet<ModelId>,
        hlc: &mut Hlc,
    ) -> Result<Captured> {
        let space = open.row.id;
        let now = now_ms();
        let links: HashMap<LibraryId, Uuid> =
            self.db.sync_links(space).await?.into_iter().collect();
        let mut out = Captured::default();

        // Collections first: memberships refer to their sync ids.
        let mut coll_ids: HashMap<CollectionId, Uuid> = HashMap::new();
        let mut local_colls = Vec::new();
        for c in self.db.collections().await? {
            let sid = self.db.collection_sync_id(space, c.id).await?;
            coll_ids.insert(c.id, sid);
            local_colls.push((sid, c.name));
        }
        let mapped_colls: HashSet<Uuid> = self
            .db
            .collection_maps(space)
            .await?
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        let published: Vec<FieldRow> = self
            .db
            .space_fields(space)
            .await?
            .into_iter()
            .filter(|r| match Key::parse(&r.key) {
                // Only collections this install has had can be deleted by it.
                Some(Key::Collection { id, .. }) => mapped_colls.contains(&id),
                _ => false,
            })
            .collect();
        let current = state::collection_state(&local_colls, now);
        for ch in state::diff(&published, &current) {
            out.push(None, &ch, hlc, now);
        }
        let local_coll_sids: HashSet<Uuid> = coll_ids.values().copied().collect();

        for &id in dirty {
            let Some((lib, path, fps)) = self.db.model_identity(id).await? else {
                continue;
            };
            let Some(&lib_sync) = links.get(&lib) else {
                continue;
            };
            let Some(data) = self.db.model_data(id).await? else {
                continue;
            };
            let org = self.db.organize(id).await?;
            let number = self.db.model_number(id).await?;
            let sid = self.db.model_sync_id(space, lib_sync, id).await?;
            let published: Vec<FieldRow> = self
                .db
                .model_fields(space, sid)
                .await?
                .into_iter()
                .filter(|r| match Key::parse(&r.key) {
                    Some(Key::Member { id, .. }) => local_coll_sids.contains(&id),
                    Some(_) => true,
                    None => false,
                })
                .collect();
            let by_key: HashMap<String, Value> = published
                .iter()
                .map(|r| (r.key.clone(), r.value.clone()))
                .collect();
            let photo_dir = self.db.photo_dir().to_path_buf();
            let mut uploads = Vec::new();
            let mut photo = |pid: PhotoId| -> Option<Value> {
                let (attempt, file) = data
                    .attempts
                    .iter()
                    .find_map(|a| a.photos.iter().find(|p| p.id == pid).map(|p| (a.id, p)))?;
                let key = Key::Photo {
                    lib: lib_sync,
                    model: sid,
                    attempt: attempt.0,
                    photo: pid.0,
                }
                .text();
                if let Some(v) = by_key.get(&key).filter(|v| !v.is_null()) {
                    return Some(v.clone());
                }
                let bytes = std::fs::read(&file.path).ok()?;
                let hash = format!("{:032x}", xxhash_rust::xxh3::xxh3_128(&bytes));
                let ext = file
                    .path
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("jpg")
                    .to_owned();
                let name = file.path.file_name()?.to_str()?.to_owned();
                uploads.push((log::photo_path(&hash, &ext), file.path.clone()));
                let _ = &photo_dir;
                Some(json!({ "hash": hash, "ext": ext, "name": name }))
            };
            let identity = Identity { path, fps };
            let cur = state::model_state(
                lib_sync,
                sid,
                &data,
                org.map_or(0, |o| o.updated_at),
                number,
                &identity,
                &coll_ids,
                &mut photo,
                now,
            );
            out.uploads.extend(uploads);
            for ch in state::diff(&published, &cur) {
                out.push(Some(sid), &ch, hlc, now);
            }
        }
        Ok(out)
    }

    /// Writes captured records as batches, uploads new photos, and keeps
    /// the rows once written.
    async fn publish(&self, open: &Open, captured: Captured) -> Result<()> {
        let space = open.row.id;
        for (path, local) in &captured.uploads {
            let bytes = match tokio::fs::read(local).await {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(path = %local.display(), error = %e, "cannot read photo for sync");
                    continue;
                }
            };
            let bytes = match &open.key {
                Some(k) => crypto::seal(k, &bytes)?,
                None => bytes,
            };
            open.store.write_once(path, bytes).await?;
        }
        if captured.records.is_empty() {
            return Ok(());
        }
        let row = self
            .db
            .sync_space(space)
            .await?
            .ok_or(SyncError::SpaceNotFound)?;
        let mut seq = row.next_seq;
        let mut since = row.since_snap;
        let observed = self.db.sync_cursors(space).await?;
        let mut rows = captured.rows;
        for (chunk_no, chunk) in captured.records.chunks(BATCH_RECORDS).enumerate() {
            let batch = Batch {
                format: FORMAT,
                install: open.me,
                seq,
                observed: observed.clone(),
                records: chunk.to_vec(),
            };
            open.store
                .write(
                    &log::batch_path(open.me, seq),
                    log::encode(&batch, open.key.as_ref())?,
                )
                .await?;
            let start = chunk_no * BATCH_RECORDS;
            let end = (start + chunk.len()).min(rows.len());
            if let Some(stamped) = rows.get_mut(start..end) {
                for r in &mut *stamped {
                    r.seq = seq;
                }
                self.db.put_fields(space, stamped).await?;
            }
            since += chunk.len() as u64;
            seq += 1;
            self.db.set_space_seq(space, seq, since).await?;
        }
        Ok(())
    }

    async fn write_device(&self, open: &Open) -> Result<()> {
        let now = now_ms();
        let due = lock(&self.sync.device_written)
            .get(&open.row.id)
            .is_none_or(|at| now - at > DEVICE_REFRESH_MS);
        if !due {
            return Ok(());
        }
        let file = DeviceFile {
            name: open.device.clone(),
            app_version: crate::VERSION.to_owned(),
            last_seen: now,
        };
        let text = toml::to_string(&file).unwrap_or_default();
        open.store
            .write(&log::device_path(open.me), text.into_bytes())
            .await?;
        lock(&self.sync.device_written).insert(open.row.id, now);
        Ok(())
    }

    // ---------------------------------------------------------------
    // Reading other installs
    // ---------------------------------------------------------------

    /// Applies another install's new snapshot and batches. Returns how many
    /// records were taken.
    async fn read_install(
        &self,
        open: &Open,
        other: Uuid,
        hlc: &mut Hlc,
        touched: &mut Touched,
    ) -> Result<usize> {
        let space = open.row.id;
        let cursor = self
            .db
            .sync_cursors(space)
            .await?
            .get(&other)
            .copied()
            .unwrap_or(0);
        let logs: Vec<u64> = {
            let mut v: Vec<u64> = open
                .store
                .list(&log::log_dir(other))
                .await?
                .iter()
                .filter_map(|n| log::parse_seq(n, "chg"))
                .filter(|s| *s > cursor)
                .collect();
            v.sort_unstable();
            v
        };
        let snaps: Vec<u64> = open
            .store
            .list(&log::snap_dir(other))
            .await?
            .iter()
            .filter_map(|n| log::parse_seq(n, "snap"))
            .collect();
        let newest_snap = snaps.iter().copied().max().unwrap_or(0);
        let first_log = logs.first().copied();
        // Read the snapshot when batches we haven't seen are gone already.
        let need_snap = newest_snap > cursor && first_log.is_none_or(|f| f > cursor + 1);
        if logs.is_empty() && !need_snap {
            return Ok(0);
        }
        self.read_device(open, other).await?;
        let mut taken = 0;
        let mut cursor = cursor;
        if need_snap {
            let bytes = open
                .store
                .read(&log::snap_path(other, newest_snap))
                .await?
                .ok_or(SyncError::Truncated)?;
            let snap = log::decode(&bytes, open.key.as_ref())?;
            taken += self.apply_batch(open, &snap, true, hlc, touched).await?;
            cursor = newest_snap;
            self.db.set_sync_cursor(space, other, cursor).await?;
        }
        for seq in logs.into_iter().filter(|s| *s > cursor) {
            let Some(bytes) = open.store.read(&log::batch_path(other, seq)).await? else {
                continue;
            };
            let batch = log::decode(&bytes, open.key.as_ref())?;
            if batch.install != other || batch.seq != seq {
                return Err(SyncError::Corrupt.into());
            }
            taken += self.apply_batch(open, &batch, false, hlc, touched).await?;
            self.db.set_sync_cursor(space, other, seq).await?;
        }
        Ok(taken)
    }

    async fn read_device(&self, open: &Open, other: Uuid) -> Result<()> {
        let Some(bytes) = open.store.read(&log::device_path(other)).await? else {
            return Ok(());
        };
        let Ok(file) = toml::from_str::<DeviceFile>(&String::from_utf8_lossy(&bytes)) else {
            return Ok(());
        };
        self.db
            .upsert_device(
                open.row.id,
                &DeviceRow {
                    install: other,
                    name: file.name.chars().take(64).collect(),
                    app_version: file.app_version.chars().take(32).collect(),
                    last_seen: file.last_seen,
                },
            )
            .await
    }

    /// Merges one batch (or snapshot) into the field rows. Local data is
    /// brought up to date afterwards, per touched model
    /// ([`Engine::apply_touched`]). A snapshot holds everything its writer
    /// knew, other installs' changes included.
    async fn apply_batch(
        &self,
        open: &Open,
        batch: &Batch,
        snapshot: bool,
        hlc: &mut Hlc,
        touched: &mut Touched,
    ) -> Result<usize> {
        let space = open.row.id;
        // Records by (model sync id) so each model's rows load once.
        let mut groups: BTreeMap<Option<(Uuid, Uuid)>, Vec<&Record>> = BTreeMap::new();
        for r in &batch.records {
            hlc.observe(r.stamp);
            if r.stamp.install != batch.install && !snapshot {
                // A batch only carries its writer's own changes.
                continue;
            }
            groups.entry(r.key.model()).or_default().push(r);
        }
        let mut taken = 0;
        for (model, recs) in groups {
            let (sid, existing): (Option<Uuid>, Vec<FieldRow>) = match model {
                Some((lib, m)) => {
                    let w = self.db.resolve_sync_id(space, lib, m).await?;
                    (Some(w), self.db.model_fields(space, w).await?)
                }
                None => (None, self.db.space_fields(space).await?),
            };
            let mut rows: HashMap<String, FieldRow> =
                existing.into_iter().map(|r| (r.key.clone(), r)).collect();
            let mut changed = Vec::new();
            for r in recs {
                let key = merge::canonical(&r.key, sid);
                let rec = Record {
                    key: key.clone(),
                    ..r.clone()
                };
                let text = key.text();
                let own = rec.stamp.install == batch.install;
                let writer = if own {
                    batch.install
                } else {
                    rec.stamp.install
                };
                let outcome = merge::decide(rows.get(&text), &rec, writer, &batch.observed);
                match outcome {
                    Outcome::Ignore => continue,
                    Outcome::Apply => {}
                    Outcome::ApplyKeepLoser { loser, loser_stamp } => {
                        touched.losers.push(Loser {
                            key: key.clone(),
                            value: loser,
                            stamp: loser_stamp,
                        });
                    }
                }
                // Where another install's change was first published isn't
                // known from a snapshot; 0 counts as seen by everyone.
                let row = merge::row_for(&rec, sid, if own { batch.seq } else { 0 });
                rows.insert(text, row.clone());
                changed.push(row);
                taken += 1;
                match (&key, model) {
                    (Key::Identity { lib, .. }, _) => {
                        touched.identities.insert((*lib, sid.unwrap_or_default()));
                    }
                    (Key::Collection { .. }, _) => touched.collections = true,
                    (_, Some((lib, _))) => {
                        touched.models.insert((lib, sid.unwrap_or_default()));
                    }
                    _ => {}
                }
            }
            if !changed.is_empty() {
                self.db.put_fields(space, &changed).await?;
            }
        }
        Ok(taken)
    }

    // ---------------------------------------------------------------
    // Bringing local data up to date
    // ---------------------------------------------------------------

    async fn apply_touched(&self, open: &Open, mut touched: Touched) -> Result<()> {
        let space = open.row.id;
        let links: HashMap<Uuid, LibraryId> = self
            .db
            .sync_links(space)
            .await?
            .into_iter()
            .map(|(l, s)| (s, l))
            .collect();
        if touched.collections {
            self.apply_collections(space).await?;
        }
        // Matching: new identities, plus parked ones when asked (first
        // sync, a link, a scan).
        let rematch = lock(&self.sync.rematch).remove(&space);
        let mut to_match: BTreeSet<(Uuid, Uuid)> = touched.identities.clone();
        if rematch {
            to_match.extend(self.db.parked(space).await?);
            for lib_sync in links.keys() {
                for sid in self.unmapped_identities(space, *lib_sync).await? {
                    to_match.insert((*lib_sync, sid));
                }
            }
        }
        let mut indexes: HashMap<Uuid, LocalIndex> = HashMap::new();
        for (lib_sync, sid) in to_match {
            let Some(&lib) = links.get(&lib_sync) else {
                continue;
            };
            if let Some(winner) = self
                .match_identity(space, lib, lib_sync, sid, &mut indexes)
                .await?
            {
                touched.models.insert((lib_sync, winner));
            }
        }
        // History of kept losing edits.
        let me = open.me;
        for l in &touched.losers {
            let Some((lib_sync, sid)) = l.key.model() else {
                continue;
            };
            let Some((Some(model), _)) = self.db.sync_model(space, lib_sync, sid).await? else {
                continue;
            };
            let device = if l.stamp.install == me {
                open.device.clone()
            } else {
                self.device_of(space, l.stamp.install).await?
            };
            self.db
                .add_edit_history(model, &l.key.text(), &l.value, &device, l.stamp.wall_ms)
                .await?;
        }
        let mut lists = touched.collections;
        for (lib_sync, sid) in touched.models {
            if !links.contains_key(&lib_sync) {
                continue;
            }
            let Some((Some(model), None)) = self.db.sync_model(space, lib_sync, sid).await? else {
                continue;
            };
            match self.rebuild(open, lib_sync, sid, model).await {
                Ok(r) => {
                    if r.changed {
                        self.sidecars.queue(model);
                        let _ = self.events.send(EngineEvent::ModelDataChanged(model));
                    }
                    lists |= r.lists;
                }
                Err(e) => tracing::warn!(model = %model, error = %e, "could not apply synced data"),
            }
        }
        if lists {
            let _ = self.events.send(EngineEvent::OrganizeChanged);
        }
        Ok(())
    }

    async fn device_of(&self, space: i64, install: Uuid) -> Result<String> {
        Ok(self
            .db
            .sync_devices(space)
            .await?
            .into_iter()
            .find(|d| d.install == install)
            .map_or_else(|| "another device".into(), |d| d.name))
    }

    /// Identity rows of a library whose model has no local mapping yet.
    async fn unmapped_identities(&self, space: i64, lib_sync: Uuid) -> Result<Vec<Uuid>> {
        let mapped: HashSet<Uuid> = self
            .db
            .mapped_models(space, lib_sync)
            .await?
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        let mut out = Vec::new();
        for r in self.db.identity_rows(space, lib_sync).await? {
            if let Some(Key::Identity { model, .. }) = Key::parse(&r.key)
                && !mapped.contains(&model)
            {
                out.push(model);
            }
        }
        Ok(out)
    }

    /// Maps sync id `sid` to a local model if one matches. Returns the
    /// winning sync id when a model's data should be rebuilt.
    async fn match_identity(
        &self,
        space: i64,
        lib: LibraryId,
        lib_sync: Uuid,
        sid: Uuid,
        indexes: &mut HashMap<Uuid, LocalIndex>,
    ) -> Result<Option<Uuid>> {
        let sid = self.db.resolve_sync_id(space, lib_sync, sid).await?;
        if let Some((Some(_), _)) = self.db.sync_model(space, lib_sync, sid).await? {
            return Ok(Some(sid));
        }
        let key = Key::Identity {
            lib: lib_sync,
            model: sid,
        };
        let Some(row) = self.db.sync_field(space, &key.text()).await? else {
            return Ok(None);
        };
        let Some(identity) = Identity::from_value(&row.value) else {
            return Ok(None);
        };
        if let std::collections::hash_map::Entry::Vacant(slot) = indexes.entry(lib_sync) {
            let models = self.db.library_identities(lib).await?;
            slot.insert(LocalIndex::new(&models));
        }
        let Some(local) = indexes[&lib_sync].match_model(&identity) else {
            // Not on this computer: parked until a scan finds it.
            self.db.map_model(space, lib_sync, sid, None).await?;
            return Ok(None);
        };
        let own = self.db.model_sync_id(space, lib_sync, local).await?;
        if own == sid {
            return Ok(Some(sid));
        }
        // The same model under two ids: the smaller id wins everywhere.
        let (win, lose) = super::identity::winner(own, sid);
        self.db.map_model(space, lib_sync, win, Some(local)).await?;
        self.db.set_alias(space, lib_sync, lose, win).await?;
        self.merge_ids(space, lose, win).await?;
        Ok(Some(win))
    }

    /// Moves the rows of a losing model sync id to the winner (newest
    /// stamp per key).
    async fn merge_ids(&self, space: i64, lose: Uuid, win: Uuid) -> Result<()> {
        let theirs = self.db.model_fields(space, lose).await?;
        if theirs.is_empty() {
            return Ok(());
        }
        let mut mine: HashMap<String, FieldRow> = self
            .db
            .model_fields(space, win)
            .await?
            .into_iter()
            .map(|r| (r.key.clone(), r))
            .collect();
        let mut out = Vec::new();
        let mut old_keys = Vec::new();
        for r in theirs {
            old_keys.push(r.key.clone());
            let Some(key) = Key::parse(&r.key) else {
                continue;
            };
            let moved = FieldRow {
                key: key.with_model(win).text(),
                model: Some(win),
                ..r
            };
            let merged = match mine.remove(&moved.key) {
                Some(m) => merge::newer(m, moved),
                None => moved,
            };
            out.push(merged);
        }
        self.db.delete_fields(space, &old_keys).await?;
        self.db.put_fields(space, &out).await?;
        Ok(())
    }

    async fn apply_collections(&self, space: i64) -> Result<()> {
        let rows = self.db.space_fields(space).await?;
        let mut names: BTreeMap<Uuid, (Option<String>, bool)> = BTreeMap::new();
        for r in rows {
            match Key::parse(&r.key) {
                Some(Key::Collection {
                    id,
                    field: CollectionField::Name,
                }) => {
                    names.entry(id).or_default().0 = r.value.as_str().map(str::to_owned);
                }
                Some(Key::Collection {
                    id,
                    field: CollectionField::Deleted,
                }) => {
                    names.entry(id).or_default().1 = r.value == json!(true);
                }
                _ => {}
            }
        }
        let locals = self.db.collections().await?;
        for (sid, (name, deleted)) in names {
            let local = self.db.local_collection(space, sid).await?;
            let exists = local.and_then(|l| locals.iter().find(|c| c.id == l));
            if deleted {
                if let Some(c) = exists {
                    self.db.delete_collection(c.id).await?;
                }
                continue;
            }
            let Some(name) = name else { continue };
            match exists {
                Some(c) if c.name != name => {
                    if let Err(e) = self.db.rename_collection(c.id, &name).await {
                        tracing::warn!(error = %e, "could not rename a synced collection");
                    }
                }
                Some(_) => {}
                None => {
                    let key = crate::model_data::name_key(&name);
                    let found = locals
                        .iter()
                        .find(|c| crate::model_data::name_key(&c.name) == key)
                        .map(|c| c.id);
                    let id = match found {
                        Some(id) => id,
                        None => match self.db.create_collection(&name).await {
                            Ok(c) => c.id,
                            Err(e) => {
                                tracing::warn!(error = %e, "could not create a synced collection");
                                continue;
                            }
                        },
                    };
                    self.db.map_collection(space, sid, Some(id)).await?;
                }
            }
        }
        Ok(())
    }

    /// Writes what the rows of model sync id `sid` say into local model
    /// `model`, where it differs.
    async fn rebuild(
        &self,
        open: &Open,
        lib_sync: Uuid,
        sid: Uuid,
        model: ModelId,
    ) -> Result<Rebuilt> {
        let space = open.row.id;
        let rows = self.db.model_fields(space, sid).await?;
        let Some(current) = self.db.model_data(model).await? else {
            return Ok(Rebuilt::default());
        };
        let want = Wanted::from_rows(&rows);
        let mut out = Rebuilt::default();

        // Notes, source, tags, collections.
        let mut coll_names = Vec::new();
        let locals = self.db.collections().await?;
        for cid in &want.members {
            if let Some(local) = self.db.local_collection(space, *cid).await?
                && let Some(c) = locals.iter().find(|c| c.id == local)
            {
                coll_names.push(c.name.clone());
            }
        }
        let org = Organize {
            notes: want.notes.clone().unwrap_or_else(|| current.notes.clone()),
            source: want
                .source
                .clone()
                .unwrap_or_else(|| current.source.clone()),
            tags: want.tags.values().cloned().collect(),
            collections: coll_names,
            updated_at: want.organize_at.max(1),
        };
        let cur_tags: BTreeSet<String> = current
            .tags
            .iter()
            .map(|t| crate::model_data::name_key(t))
            .collect();
        let want_tags: BTreeSet<String> = want.tags.keys().cloned().collect();
        let cur_colls: BTreeSet<String> =
            current.collections.iter().map(|c| c.name.clone()).collect();
        let want_colls: BTreeSet<String> = org.collections.iter().cloned().collect();
        if org.notes != current.notes
            || org.source != current.source
            || cur_tags != want_tags
            || cur_colls != want_colls
        {
            out.lists |= cur_tags != want_tags || cur_colls != want_colls;
            self.db.import_organize(model, &org).await?;
            out.changed = true;
        }

        // Recipe.
        match (&want.recipe, &current.recipe) {
            (WantRecipe::Deleted, Some(_)) => {
                self.db.set_recipe(model, None).await?;
                out.changed = true;
            }
            (WantRecipe::Set(r), cur) => {
                let same = cur.as_ref().is_some_and(|c| {
                    c.settings == r.settings
                        && c.required == r.required
                        && c.notes == r.notes
                        && c.parts == r.parts
                        && c.from_attempt == r.from_attempt
                });
                if !same {
                    match self.db.import_recipe(model, r).await {
                        Ok(()) => out.changed = true,
                        Err(e) => tracing::warn!(error = %e, "skipping an invalid synced recipe"),
                    }
                }
            }
            _ => {}
        }

        // Attempts and photos.
        for (aid, w) in &want.attempts {
            let cur = current.attempts.iter().find(|a| a.id.0 == *aid);
            if w.deleted {
                if cur.is_some() {
                    let (_, photos) = self.db.delete_attempt(AttemptId(*aid)).await?;
                    let paths: Vec<_> = photos.into_iter().map(|p| p.path).collect();
                    let _ =
                        tokio::task::spawn_blocking(move || crate::photos::delete_files(&paths))
                            .await;
                    out.changed = true;
                }
                continue;
            }
            let Some(a) = w.attempt(*aid) else { continue };
            let same = cur.is_some_and(|c| {
                c.printed_at == a.printed_at
                    && c.result == a.result
                    && c.settings == a.settings
                    && c.duration_min == a.duration_min
                    && c.notes == a.notes
                    && c.parts == a.parts
            });
            if !same {
                if self.db.apply_attempt(model, &a).await? {
                    out.changed = true;
                } else {
                    continue;
                }
            }
            let have: HashSet<Uuid> = cur
                .map(|c| c.photos.iter().map(|p| p.id.0).collect())
                .unwrap_or_default();
            for (pid, photo) in &w.photos {
                match photo {
                    Some(p) if !have.contains(pid) => {
                        if self
                            .fetch_photo(open, AttemptId(*aid), PhotoId(*pid), p)
                            .await?
                        {
                            out.changed = true;
                        }
                    }
                    None if have.contains(pid) => {
                        if let Ok((_, r)) = self.db.remove_photo_row(PhotoId(*pid)).await {
                            let p = vec![r.path];
                            let _ = tokio::task::spawn_blocking(move || {
                                crate::photos::delete_files(&p)
                            })
                            .await;
                            out.changed = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        // Catalogue number.
        if let Some((n, stamp)) = want.number {
            let lib = self
                .db
                .model_library(model)
                .await?
                .ok_or(CoreError::ModelNotFound)?;
            let cur = self.db.model_number(model).await?;
            if cur != Some(n) {
                match self.db.number_holder(lib, n).await? {
                    None => {
                        self.db.force_number(model, n).await?;
                        out.changed = true;
                    }
                    Some(holder) => {
                        let holder_stamp = self.number_stamp(space, lib_sync, holder).await?;
                        match merge::resolve_number(holder_stamp, stamp) {
                            NumberOutcome::HolderKeeps => {
                                // This model gets a new number, published as
                                // this install's change.
                                if cur.is_none() {
                                    self.db.renumber(model).await?;
                                }
                                self.mark_sync_dirty(model);
                            }
                            NumberOutcome::IncomingTakes => {
                                self.db.clear_number(holder).await?;
                                self.db.force_number(model, n).await?;
                                self.db.renumber(holder).await?;
                                self.mark_sync_dirty(holder);
                                self.sidecars.queue(holder);
                                let _ = self.events.send(EngineEvent::ModelDataChanged(holder));
                                out.changed = true;
                            }
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    async fn number_stamp(
        &self,
        space: i64,
        lib_sync: Uuid,
        model: ModelId,
    ) -> Result<Option<Stamp>> {
        let sid = self.db.model_sync_id(space, lib_sync, model).await?;
        let key = Key::Model {
            lib: lib_sync,
            model: sid,
            field: ModelField::Number,
        };
        Ok(self
            .db
            .sync_field(space, &key.text())
            .await?
            .map(|r| r.stamp))
    }

    /// Downloads a synced photo into the photo folder and records it.
    async fn fetch_photo(
        &self,
        open: &Open,
        attempt: AttemptId,
        photo: PhotoId,
        p: &WantPhoto,
    ) -> Result<bool> {
        let path = log::photo_path(&p.hash, &p.ext);
        let Some(bytes) = open.store.read(&path).await? else {
            tracing::info!(photo = %photo, "synced photo not uploaded yet");
            return Ok(false);
        };
        let bytes = match &open.key {
            Some(k) => crypto::open(k, &bytes)?,
            None => bytes,
        };
        let head = bytes.get(..32).unwrap_or(&bytes);
        if bytes.len() as u64 > crate::photos::MAX_PHOTO_BYTES
            || crate::photos::photo_extension(head).is_none()
        {
            return Ok(false);
        }
        let ext = crate::photos::photo_extension(head).unwrap_or("jpg");
        let name = format!("{photo}.{ext}");
        let dir = self.db.photo_dir().join(attempt.to_string());
        let dest = dir.join(&name);
        let write = {
            let dest = dest.clone();
            tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&dir)?;
                crate::fsx::durable_write(&dest, &bytes, crate::fsx::Durability::UserData)
            })
        };
        match write.await {
            Ok(Ok(())) => {}
            Ok(Err(source)) => return Err(CoreError::Io { path: dest, source }),
            Err(_) => return Ok(false),
        }
        match self.db.add_photo_row(attempt, photo, &name).await {
            Ok(_) => Ok(true),
            Err(e) => {
                tracing::warn!(error = %e, "could not record a synced photo");
                let p = vec![dest];
                let _ = tokio::task::spawn_blocking(move || crate::photos::delete_files(&p)).await;
                Ok(false)
            }
        }
    }

    // ---------------------------------------------------------------
    // Compaction
    // ---------------------------------------------------------------

    /// Writes a snapshot of every field this install knows (whoever wrote
    /// it: ids merged on this install carry others' stamps) and removes
    /// its batches the previous snapshot already covered.
    pub(crate) async fn compact(&self, open: &Open, row: &SpaceRow) -> Result<()> {
        let space = row.id;
        let last = row.next_seq.saturating_sub(1);
        if last == 0 || last <= row.snap_seq {
            return Ok(());
        }
        let records: Vec<Record> = self
            .db
            .all_fields(space)
            .await?
            .into_iter()
            .filter_map(|r| {
                Some(Record {
                    stamp: r.stamp,
                    key: Key::parse(&r.key)?,
                    value: r.value,
                })
            })
            .collect();
        let snap = Batch {
            format: FORMAT,
            install: open.me,
            seq: last,
            observed: self.db.sync_cursors(space).await?,
            records,
        };
        open.store
            .write(
                &log::snap_path(open.me, last),
                log::encode(&snap, open.key.as_ref())?,
            )
            .await?;
        // Keep one generation back for installs reading right now.
        let prev = row.snap_seq;
        for name in open.store.list(&log::log_dir(open.me)).await? {
            if log::parse_seq(&name, "chg").is_some_and(|s| s <= prev) {
                open.store
                    .delete(&format!("{}/{name}", log::log_dir(open.me)))
                    .await?;
            }
        }
        for name in open.store.list(&log::snap_dir(open.me)).await? {
            if log::parse_seq(&name, "snap").is_some_and(|s| s < prev) {
                open.store
                    .delete(&format!("{}/{name}", log::snap_dir(open.me)))
                    .await?;
            }
        }
        self.db.set_space_snap(space, last).await?;
        Ok(())
    }
}

/// `<library>/.printdex/sync`
pub fn library_space_dir(root: &std::path::Path) -> std::path::PathBuf {
    root.join(".printdex").join("sync")
}

pub(crate) fn parse_space_file(bytes: &[u8]) -> Result<SpaceFile> {
    let file: SpaceFile =
        toml::from_str(&String::from_utf8_lossy(bytes)).map_err(|_| SyncError::Corrupt)?;
    if file.format > FORMAT {
        return Err(SyncError::NewerFormat(file.format).into());
    }
    Ok(file)
}

/// Derives a space key off the async workers and checks it.
pub(crate) async fn derive_checked(pass: &str, file: &SpaceFile) -> Result<SpaceKey> {
    let kdf = file.kdf.clone().ok_or(SyncError::Corrupt)?;
    let check = file.check.clone().ok_or(SyncError::Corrupt)?;
    let pass = pass.to_owned();
    let key = tokio::task::spawn_blocking(move || crypto::derive(&pass, &kdf))
        .await
        .map_err(|e| SyncError::Storage(e.to_string()))??;
    crypto::verify(&key, &check)?;
    Ok(key)
}

/// What a capture found.
#[derive(Debug, Default)]
pub(crate) struct Captured {
    pub records: Vec<Record>,
    /// The rows they leave (seq filled in when written).
    pub rows: Vec<FieldRow>,
    /// Photos to upload: path in the space, local file.
    pub uploads: Vec<(String, std::path::PathBuf)>,
}

impl Captured {
    fn push(&mut self, model: Option<Uuid>, ch: &state::Change, hlc: &mut Hlc, now: i64) {
        let rec = Record {
            stamp: state::stamp(ch, hlc, now),
            key: ch.key.clone(),
            value: ch.value.clone(),
        };
        self.rows.push(merge::row_for(&rec, model, 0));
        self.records.push(rec);
    }
}

#[derive(Debug, Default)]
struct Touched {
    /// (library sync id, model sync id)
    models: BTreeSet<(Uuid, Uuid)>,
    identities: BTreeSet<(Uuid, Uuid)>,
    collections: bool,
    losers: Vec<Loser>,
}

#[derive(Debug)]
struct Loser {
    key: Key,
    value: Value,
    stamp: Stamp,
}

#[derive(Debug, Default)]
struct Rebuilt {
    changed: bool,
    lists: bool,
}

#[derive(Debug, Default)]
enum WantRecipe {
    #[default]
    Unknown,
    Deleted,
    Set(Box<Recipe>),
}

#[derive(Debug, Clone, PartialEq)]
struct WantPhoto {
    hash: String,
    ext: String,
}

#[derive(Debug, Default)]
struct WantAttempt {
    deleted: bool,
    printed_at: Option<i64>,
    result: Option<AttemptResult>,
    settings: PrintSettings,
    duration_min: Option<u32>,
    notes: String,
    parts: Vec<String>,
    updated_at: i64,
    photos: BTreeMap<Uuid, Option<WantPhoto>>,
}

impl WantAttempt {
    fn attempt(&self, id: Uuid) -> Option<Attempt> {
        Some(Attempt {
            id: AttemptId(id),
            printed_at: self.printed_at?,
            result: self.result?,
            settings: self.settings.clone(),
            duration_min: self.duration_min,
            notes: self.notes.clone(),
            parts: self.parts.clone(),
            photos: Vec::new(),
            number: 0,
            updated_at: self.updated_at.max(1),
        })
    }
}

/// What a model's rows say it should look like.
#[derive(Debug, Default)]
struct Wanted {
    notes: Option<String>,
    source: Option<Option<String>>,
    /// name key → as typed
    tags: BTreeMap<String, String>,
    members: BTreeSet<Uuid>,
    organize_at: i64,
    number: Option<(u32, Stamp)>,
    recipe: WantRecipe,
    attempts: BTreeMap<Uuid, WantAttempt>,
}

impl Wanted {
    fn from_rows(rows: &[FieldRow]) -> Wanted {
        let mut w = Wanted::default();
        let mut recipe = Recipe::default();
        let mut recipe_deleted = None;
        let mut recipe_seen = false;
        for r in rows {
            let Some(key) = Key::parse(&r.key) else {
                continue;
            };
            let v = &r.value;
            match key {
                Key::Model { field, .. } => {
                    w.organize_at = w.organize_at.max(r.stamp.wall_ms);
                    match field {
                        ModelField::Notes => {
                            w.notes = Some(v.as_str().unwrap_or_default().to_owned())
                        }
                        ModelField::Source => w.source = Some(v.as_str().map(str::to_owned)),
                        ModelField::Number => {
                            if let Some(n) = v
                                .as_u64()
                                .and_then(|n| u32::try_from(n).ok())
                                .filter(|n| *n > 0)
                            {
                                w.number = Some((n, r.stamp));
                            }
                        }
                    }
                }
                Key::Tag { tag, .. } => {
                    w.organize_at = w.organize_at.max(r.stamp.wall_ms);
                    if let Some(name) = v.as_str() {
                        w.tags.insert(tag, name.to_owned());
                    }
                }
                Key::Member { id, .. } => {
                    w.organize_at = w.organize_at.max(r.stamp.wall_ms);
                    if v == &json!(true) {
                        w.members.insert(id);
                    }
                }
                Key::Recipe { field, .. } => {
                    recipe.updated_at = recipe.updated_at.max(r.stamp.wall_ms);
                    match field {
                        RecipeField::Deleted => recipe_deleted = Some(v == &json!(true)),
                        RecipeField::Settings => {
                            recipe_seen = true;
                            recipe.settings = serde_json::from_value(v.clone()).unwrap_or_default();
                        }
                        RecipeField::Required => {
                            recipe_seen = true;
                            recipe.required = serde_json::from_value(v.clone()).unwrap_or_default();
                        }
                        RecipeField::Notes => {
                            recipe_seen = true;
                            recipe.notes = serde_json::from_value(v.clone()).unwrap_or_default();
                        }
                        RecipeField::Parts => {
                            recipe_seen = true;
                            recipe.parts = serde_json::from_value(v.clone()).unwrap_or_default();
                        }
                        RecipeField::FromAttempt => {
                            recipe.from_attempt = v
                                .as_str()
                                .and_then(|s| Uuid::parse_str(s).ok())
                                .map(AttemptId);
                        }
                    }
                }
                Key::Attempt { attempt, field, .. } => {
                    let a = w.attempts.entry(attempt).or_default();
                    a.updated_at = a.updated_at.max(r.stamp.wall_ms);
                    match field {
                        AttemptField::Deleted => a.deleted = v == &json!(true),
                        AttemptField::PrintedAt => a.printed_at = v.as_i64(),
                        AttemptField::Result => a.result = serde_json::from_value(v.clone()).ok(),
                        AttemptField::Settings => {
                            a.settings = serde_json::from_value(v.clone()).unwrap_or_default();
                        }
                        AttemptField::DurationMin => {
                            a.duration_min = v.as_u64().and_then(|d| u32::try_from(d).ok());
                        }
                        AttemptField::Notes => a.notes = v.as_str().unwrap_or_default().to_owned(),
                        AttemptField::Parts => {
                            a.parts = serde_json::from_value(v.clone()).unwrap_or_default()
                        }
                    }
                }
                Key::Photo { attempt, photo, .. } => {
                    let p = v.as_object().and_then(|o| {
                        let hash = o.get("hash")?.as_str()?;
                        let ext = o.get("ext")?.as_str()?;
                        // Only content hashes and image extensions, nothing
                        // that could leave the photos folder.
                        (hash.len() == 32
                            && hash.bytes().all(|b| b.is_ascii_hexdigit())
                            && matches!(ext, "png" | "jpg" | "webp"))
                        .then(|| WantPhoto {
                            hash: hash.to_owned(),
                            ext: ext.to_owned(),
                        })
                    });
                    w.attempts
                        .entry(attempt)
                        .or_default()
                        .photos
                        .insert(photo, p);
                }
                Key::Identity { .. } | Key::Collection { .. } => {}
            }
        }
        w.recipe = match recipe_deleted {
            Some(true) => WantRecipe::Deleted,
            Some(false) | None if recipe_seen => WantRecipe::Set(Box::new(recipe)),
            _ => WantRecipe::Unknown,
        };
        w
    }
}

impl Db {
    /// Identity rows of library `lib_sync`.
    pub(crate) async fn identity_rows(&self, space: i64, lib_sync: Uuid) -> Result<Vec<FieldRow>> {
        let prefix = format!("{{\"identity\":{{\"lib\":\"{}\"", lib_sync.hyphenated());
        let all = self.space_identity_rows(space, &prefix).await?;
        Ok(all)
    }
}
