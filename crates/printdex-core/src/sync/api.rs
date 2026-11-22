//! Engine commands for sync: spaces, links, status, parked models,
//! "Edited elsewhere" and the scheduler.

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use uuid::Uuid;

use super::crypto::{self, KdfParams};
use super::log::{AttemptField, FORMAT, Key, ModelField, RecipeField};
use super::run::{
    LibraryFile, SPACE_FILE, SpaceFile, derive_checked, library_path, library_space_dir,
    parse_space_file, pass_key, secret_key,
};
use super::state::Identity;
use super::store::{FolderStore, SpaceStore};
use crate::backup::{Target, TargetSecrets};
use crate::db::{DeviceRow, EditEntry, SpaceKind, SpaceRow};
use crate::engine::Engine;
use crate::error::{CoreError, Result, SyncError};
use crate::events::EngineEvent;
use crate::ids::{AttemptId, LibraryId, ModelId};
use crate::time::now_ms;

/// Wait after the last local change before syncing.
pub const DEBOUNCE: Duration = Duration::from_secs(10);
/// Sync at least this often while Printdex is open.
pub const INTERVAL: Duration = Duration::from_secs(5 * 60);
/// A device must have been away this long before it can be forgotten.
pub const FORGET_AFTER_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// A space and how its syncing goes.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceInfo {
    pub id: i64,
    pub kind: SpaceKind,
    pub name: String,
    pub location: String,
    pub encrypted: bool,
    /// Local libraries linked in this space and their sync ids.
    pub libraries: Vec<(LibraryId, Uuid)>,
    /// Other installs seen in the space.
    pub devices: Vec<DeviceRow>,
    pub last_sync_at: Option<i64>,
    pub last_error: Option<String>,
    /// Models another install has that aren't on this computer.
    pub parked: usize,
    /// The passphrase is needed before this space can sync.
    pub locked: bool,
}

/// A library published in a target space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteLibrary {
    pub id: Uuid,
    pub name: String,
    pub created_by: String,
    /// The local library already linked to it.
    pub linked: Option<LibraryId>,
    /// A local library that holds many of the same files.
    pub suggested: Option<LibraryId>,
}

/// A model another install has that this computer doesn't.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParkedModel {
    pub path: String,
    pub device: String,
}

/// Which device last changed a model's notes and attempts, when it wasn't
/// this one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncAttribution {
    pub notes: Option<String>,
    pub attempts: HashMap<AttemptId, String>,
}

impl Engine {
    async fn space_row(&self, id: i64) -> Result<SpaceRow> {
        Ok(self
            .db
            .sync_space(id)
            .await?
            .ok_or(SyncError::SpaceNotFound)?)
    }

    /// Creates or joins the space of a shared library
    /// (`<library>/.printdex/sync`). A non-empty passphrase encrypts a new
    /// space; joining an encrypted one needs its passphrase.
    pub async fn add_library_space(&self, lib: LibraryId, passphrase: &str) -> Result<SpaceInfo> {
        let library = self.db.library(lib).await?;
        for s in self.db.sync_spaces().await? {
            if s.kind == SpaceKind::Library && s.library_id == Some(lib) {
                return Err(SyncError::AlreadyLinked.into());
            }
        }
        let store = FolderStore::new(library_space_dir(&library.path));
        let (file, created) = self.create_or_join(&store, passphrase, true).await?;
        let lib_sync = file.library.ok_or(SyncError::Corrupt)?;
        let id = self
            .db
            .insert_space(
                SpaceKind::Library,
                Some(lib),
                None,
                &library.display_name,
                file.space_id,
                file.kdf.is_some(),
                now_ms(),
            )
            .await?;
        self.after_add(id, &file, passphrase, true).await?;
        if created {
            self.write_library_file(&store, lib_sync, &library.display_name)
                .await?;
        }
        self.link_in(id, lib, lib_sync).await?;
        self.space_info(id).await
    }

    /// Creates or joins a space on a folder or remote target.
    pub async fn add_target_space(
        &self,
        target: Target,
        secrets: &TargetSecrets,
        passphrase: &str,
        remember: bool,
    ) -> Result<SpaceInfo> {
        let limit = self.backup_config().await?.schedule.upload_limit_kib;
        let store = super::store::open_store(&target, secrets, limit)?;
        let (file, _) = self
            .create_or_join(store.as_ref(), passphrase, false)
            .await?;
        let json = serde_json::to_string(&target).map_err(|_| SyncError::Corrupt)?;
        let id = self
            .db
            .insert_space(
                SpaceKind::Target,
                None,
                Some(&json),
                &store.location(),
                file.space_id,
                file.kdf.is_some(),
                now_ms(),
            )
            .await?;
        if let Some(s) = &secrets.secret {
            self.secrets.set(&secret_key(id), s, remember);
        }
        self.after_add(id, &file, passphrase, remember).await?;
        self.space_info(id).await
    }

    async fn after_add(
        &self,
        id: i64,
        file: &SpaceFile,
        passphrase: &str,
        remember: bool,
    ) -> Result<()> {
        if file.kdf.is_some() {
            self.secrets.set(&pass_key(id), passphrase, remember);
        }
        self.sync.rematch_all([id]);
        self.sync.poke.notify_one();
        Ok(())
    }

    /// Reads `space.toml`, or writes a new one. Returns it and whether it
    /// was created.
    async fn create_or_join(
        &self,
        store: &dyn SpaceStore,
        passphrase: &str,
        with_library: bool,
    ) -> Result<(SpaceFile, bool)> {
        for _ in 0..2 {
            if let Some(bytes) = store.read(SPACE_FILE).await? {
                let file = parse_space_file(&bytes)?;
                if file.kdf.is_some() {
                    if passphrase.is_empty() {
                        return Err(SyncError::PassphraseNeeded.into());
                    }
                    derive_checked(passphrase, &file).await?;
                }
                return Ok((file, false));
            }
            let (kdf, check) = if passphrase.is_empty() {
                (None, None)
            } else {
                let kdf = KdfParams::generate();
                let (k, p) = (kdf.clone(), passphrase.to_owned());
                let key = tokio::task::spawn_blocking(move || crypto::derive(&p, &k))
                    .await
                    .map_err(|e| SyncError::Storage(e.to_string()))??;
                (Some(kdf), Some(crypto::check_value(&key)))
            };
            let file = SpaceFile {
                format: FORMAT,
                space_id: Uuid::now_v7(),
                created_at: now_ms(),
                library: with_library.then(Uuid::now_v7),
                kdf,
                check,
            };
            let text = toml::to_string(&file).map_err(|_| SyncError::Corrupt)?;
            if store.write_once(SPACE_FILE, text.into_bytes()).await? {
                return Ok((file, true));
            }
            // Another install created it at the same moment: join that.
        }
        Err(SyncError::Corrupt.into())
    }

    async fn write_library_file(&self, store: &dyn SpaceStore, id: Uuid, name: &str) -> Result<()> {
        let file = LibraryFile {
            id,
            name: name.to_owned(),
            created_by: self.device_name().await?,
        };
        let text = toml::to_string(&file).map_err(|_| SyncError::Corrupt)?;
        store
            .write_once(&library_path(id), text.into_bytes())
            .await?;
        Ok(())
    }

    async fn link_in(&self, space: i64, lib: LibraryId, lib_sync: Uuid) -> Result<()> {
        self.db.link_library(space, lib, lib_sync).await?;
        // Publish everything this library already has.
        let ids = self.db.library_model_ids(lib).await?;
        self.db.mark_sync_dirty(&ids).await?;
        self.sync.rematch_all([space]);
        self.sync.poke.notify_one();
        Ok(())
    }

    /// Unlocks an encrypted space with its passphrase.
    pub async fn unlock_space(&self, space: i64, passphrase: &str, remember: bool) -> Result<()> {
        let row = self.space_row(space).await?;
        let store = self.space_store(&row).await?;
        let bytes = store
            .read(SPACE_FILE)
            .await?
            .ok_or(SyncError::Unreachable("space.toml is missing".into()))?;
        let file = parse_space_file(&bytes)?;
        let key = derive_checked(passphrase, &file).await?;
        self.sync
            .keys
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(space, key);
        self.secrets.set(&pass_key(space), passphrase, remember);
        self.sync.poke.notify_one();
        Ok(())
    }

    /// Libraries published in a space, with a suggested local match.
    pub async fn space_libraries(&self, space: i64) -> Result<Vec<RemoteLibrary>> {
        let row = self.space_row(space).await?;
        let store = self.space_store(&row).await?;
        let links = self.db.sync_links(space).await?;
        let mut out = Vec::new();
        let mut local_fps: Option<Vec<(LibraryId, BTreeSet<u64>)>> = None;
        for name in store.list("libraries").await? {
            let Some(bytes) = store.read(&format!("libraries/{name}")).await? else {
                continue;
            };
            let Ok(file) = toml::from_str::<LibraryFile>(&String::from_utf8_lossy(&bytes)) else {
                continue;
            };
            let linked = links.iter().find(|(_, s)| *s == file.id).map(|(l, _)| *l);
            let mut suggested = None;
            if linked.is_none() {
                let remote: BTreeSet<u64> = self
                    .db
                    .identity_rows(space, file.id)
                    .await?
                    .iter()
                    .filter_map(|r| Identity::from_value(&r.value))
                    .flat_map(|i| i.fps)
                    .collect();
                if !remote.is_empty() {
                    if local_fps.is_none() {
                        let mut v = Vec::new();
                        for lib in self.db.libraries().await? {
                            let fps = self
                                .db
                                .library_identities(lib.id)
                                .await?
                                .into_iter()
                                .flat_map(|m| m.2)
                                .collect();
                            v.push((lib.id, fps));
                        }
                        local_fps = Some(v);
                    }
                    suggested = local_fps
                        .as_ref()
                        .into_iter()
                        .flatten()
                        .filter(|(l, _)| !links.iter().any(|(ll, _)| ll == l))
                        .map(|(l, fps)| (*l, super::identity::library_overlap(&remote, fps)))
                        .filter(|(_, o)| *o >= super::identity::SUGGEST_OVERLAP)
                        .max_by(|a, b| a.1.total_cmp(&b.1))
                        .map(|(l, _)| l);
                }
            }
            out.push(RemoteLibrary {
                id: file.id,
                name: file.name,
                created_by: file.created_by,
                linked,
                suggested,
            });
        }
        Ok(out)
    }

    /// Links local library `lib` to library `remote` of a target space, or
    /// publishes it there as a new one (`None`).
    pub async fn link_library(
        &self,
        space: i64,
        lib: LibraryId,
        remote: Option<Uuid>,
    ) -> Result<()> {
        let row = self.space_row(space).await?;
        if row.kind == SpaceKind::Library {
            return Err(SyncError::AlreadyLinked.into());
        }
        let links = self.db.sync_links(space).await?;
        if links.iter().any(|(l, s)| *l == lib || Some(*s) == remote) {
            return Err(SyncError::AlreadyLinked.into());
        }
        let library = self.db.library(lib).await?;
        let lib_sync = match remote {
            Some(id) => id,
            None => {
                let id = Uuid::now_v7();
                let store = self.space_store(&row).await?;
                self.write_library_file(store.as_ref(), id, &library.display_name)
                    .await?;
                id
            }
        };
        self.link_in(space, lib, lib_sync).await
    }

    /// Stops syncing a library in a space. Local data stays.
    pub async fn unlink_library(&self, space: i64, lib: LibraryId) -> Result<()> {
        if !self.db.unlink_library(space, lib).await? {
            return Err(SyncError::NotLinked.into());
        }
        Ok(())
    }

    /// Leaves a space. Local data stays and nothing in the space is
    /// deleted.
    pub async fn remove_space(&self, space: i64) -> Result<()> {
        self.space_row(space).await?;
        self.db.remove_space(space).await?;
        self.secrets.delete(&pass_key(space));
        self.secrets.delete(&secret_key(space));
        self.sync
            .keys
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&space);
        Ok(())
    }

    /// Every space and how its syncing goes.
    pub async fn sync_status(&self) -> Result<Vec<SpaceInfo>> {
        let mut out = Vec::new();
        for s in self.db.sync_spaces().await? {
            out.push(self.space_info(s.id).await?);
        }
        Ok(out)
    }

    pub async fn space_info(&self, id: i64) -> Result<SpaceInfo> {
        let row = self.space_row(id).await?;
        let location = match row.kind {
            SpaceKind::Library => match row.library_id {
                Some(l) => library_space_dir(&self.db.library(l).await?.path)
                    .display()
                    .to_string(),
                None => String::new(),
            },
            SpaceKind::Target => row.name.clone(),
        };
        let locked = row.encrypted
            && !self
                .sync
                .keys
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains_key(&id)
            && self.secrets.get(&pass_key(id)).is_none();
        Ok(SpaceInfo {
            id,
            kind: row.kind,
            name: row.name.clone(),
            location,
            encrypted: row.encrypted,
            libraries: self.db.sync_links(id).await?,
            devices: self.db.sync_devices(id).await?,
            last_sync_at: row.last_sync_at,
            last_error: row.last_error.clone(),
            parked: self.parked_models(id).await?.len(),
            locked,
        })
    }

    /// Models other installs have that aren't on this computer.
    pub async fn parked_models(&self, space: i64) -> Result<Vec<ParkedModel>> {
        let devices = self.db.sync_devices(space).await?;
        let mut out = Vec::new();
        for (lib_sync, sid) in self.db.parked(space).await? {
            let key = Key::Identity {
                lib: lib_sync,
                model: sid,
            };
            let Some(row) = self.db.sync_field(space, &key.text()).await? else {
                continue;
            };
            let Some(identity) = Identity::from_value(&row.value) else {
                continue;
            };
            let device = devices
                .iter()
                .find(|d| d.install == row.stamp.install)
                .map_or_else(|| "another device".into(), |d| d.name.clone());
            out.push(ParkedModel {
                path: identity.path,
                device,
            });
        }
        // A model two installs published under two ids shows once.
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out.dedup_by(|a, b| a.path == b.path);
        Ok(out)
    }

    /// Forgets a device that hasn't synced for 30 days and removes its
    /// files from the space.
    pub async fn forget_device(&self, space: i64, install: Uuid) -> Result<()> {
        let row = self.space_row(space).await?;
        let device = self
            .db
            .sync_devices(space)
            .await?
            .into_iter()
            .find(|d| d.install == install)
            .ok_or(SyncError::SpaceNotFound)?;
        if now_ms() - device.last_seen < FORGET_AFTER_MS {
            return Err(SyncError::DeviceRecent.into());
        }
        let store = self.space_store(&row).await?;
        let base = format!("installs/{}", install.hyphenated());
        for dir in [super::log::log_dir(install), super::log::snap_dir(install)] {
            for name in store.list(&dir).await? {
                store.delete(&format!("{dir}/{name}")).await?;
            }
        }
        store.delete(&format!("{base}/device.toml")).await?;
        self.db.remove_device(space, install).await?;
        Ok(())
    }

    /// "Edited elsewhere" entries of a model, newest first.
    pub async fn edit_history(&self, model: ModelId) -> Result<Vec<EditEntry>> {
        self.db.edit_history(model).await
    }

    /// Puts a kept value back (as a new change of this install).
    pub async fn restore_edit(&self, id: Uuid) -> Result<()> {
        let e = self
            .db
            .edit_entry(id)
            .await?
            .ok_or(CoreError::ModelNotFound)?;
        let text = e.value.as_str().unwrap_or_default().to_owned();
        match Key::parse(&e.key) {
            Some(Key::Model {
                field: ModelField::Notes,
                ..
            }) => self.set_notes(e.model, &text).await?,
            Some(Key::Recipe {
                field: RecipeField::Notes,
                ..
            }) => {
                let notes = serde_json::from_value(e.value.clone()).unwrap_or_default();
                let mut r = self
                    .db
                    .model_data(e.model)
                    .await?
                    .and_then(|d| d.recipe)
                    .unwrap_or_default();
                r.notes = notes;
                self.set_recipe(e.model, Some(&r)).await?;
            }
            Some(Key::Attempt {
                attempt,
                field: AttemptField::Notes,
                ..
            }) => {
                let (_, mut a) = self.db.attempt(AttemptId(attempt)).await?;
                a.notes = text;
                self.update_attempt(&a).await?;
            }
            _ => {}
        }
        self.db.dismiss_edit(id).await
    }

    pub async fn dismiss_edit(&self, id: Uuid) -> Result<()> {
        self.db.dismiss_edit(id).await
    }

    /// Which other device last changed this model's notes and attempts.
    pub async fn sync_attribution(&self, model: ModelId) -> Result<SyncAttribution> {
        let mut out = SyncAttribution::default();
        let Some(lib) = self.db.model_library(model).await? else {
            return Ok(out);
        };
        for space in self.db.spaces_of_library(lib).await? {
            let Some(lib_sync) = self
                .db
                .sync_links(space)
                .await?
                .into_iter()
                .find(|(l, _)| *l == lib)
                .map(|(_, s)| s)
            else {
                continue;
            };
            let sid = self.db.model_sync_id(space, lib_sync, model).await?;
            let devices = self.db.sync_devices(space).await?;
            let name = |i: Uuid| {
                devices
                    .iter()
                    .find(|d| d.install == i)
                    .map(|d| d.name.clone())
            };
            let mut newest: HashMap<AttemptId, super::Stamp> = HashMap::new();
            let mut notes_stamp = None;
            for r in self.db.model_fields(space, sid).await? {
                match Key::parse(&r.key) {
                    Some(Key::Model {
                        field: ModelField::Notes,
                        ..
                    }) => notes_stamp = Some(r.stamp),
                    Some(Key::Attempt { attempt, .. }) => {
                        let e = newest.entry(AttemptId(attempt)).or_insert(r.stamp);
                        if r.stamp > *e {
                            *e = r.stamp;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(s) = notes_stamp.filter(|s| s.install != self.install) {
                out.notes = name(s.install);
            }
            for (a, s) in newest {
                if s.install != self.install
                    && let Some(n) = name(s.install)
                {
                    out.attempts.insert(a, n);
                }
            }
        }
        Ok(out)
    }

    /// Syncs at start, 10 s after the last local change, and every five
    /// minutes while the engine runs. Calling it again does nothing.
    pub fn start_sync_scheduler(&self) {
        if self
            .sync
            .scheduler
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            return;
        }
        let engine = self.clone();
        let mut events = self.events.subscribe();
        tokio::spawn(async move {
            let mut next_full = tokio::time::Instant::now();
            loop {
                let wait = next_full.saturating_duration_since(tokio::time::Instant::now());
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = engine.sync.poke.notified() => {
                        // Debounce: wait until changes stop for a moment.
                        loop {
                            tokio::select! {
                                _ = tokio::time::sleep(DEBOUNCE) => break,
                                _ = engine.sync.poke.notified() => {}
                            }
                        }
                    }
                    ev = events.recv() => {
                        if let Ok(EngineEvent::ScanFinished { .. }) = ev {
                            // A scan may have brought models that were parked.
                            let ids: Vec<i64> = engine
                                .db
                                .sync_spaces()
                                .await
                                .map(|s| s.into_iter().map(|s| s.id).collect())
                                .unwrap_or_default();
                            engine.sync.rematch_all(ids);
                            engine.sync.poke.notify_one();
                        }
                        continue;
                    }
                }
                if let Err(e) = engine.sync_now().await {
                    tracing::warn!(error = %e, "sync run failed");
                }
                next_full = tokio::time::Instant::now() + INTERVAL;
            }
        });
    }
}
