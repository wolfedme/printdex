//! The engine handle: the one entry point for the app and the CLI.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::db::{
    ChangeMode, Db, Library, LibraryKind, ModelDetail, ModelHit, Scope, SearchFilter, SmartCounts,
    SortOrder, Stats,
};
use crate::error::{CoreError, Result};
use crate::events::EngineEvent;
use crate::fs::{Fs, RealFs};
use crate::ids::{LibraryId, ModelId};
use crate::scan::{ScanSummary, SidecarCtx, scan_library, scan_subtrees};
use crate::sidecar::SidecarWriter;
use crate::thumbs::ThumbnailService;

/// Event channel capacity. Slow subscribers miss old events (`Lagged`).
const EVENT_CAPACITY: usize = 1024;

/// How long a reachability check may take before the library counts as offline.
pub const STATUS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Whether a library's folder is reachable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryStatus {
    Online,
    Offline,
}

pub(crate) async fn folder_status(fs: Arc<dyn Fs>, path: PathBuf) -> LibraryStatus {
    let check = tokio::task::spawn_blocking(move || fs.read_dir(&path).is_ok());
    match tokio::time::timeout(STATUS_TIMEOUT, check).await {
        Ok(Ok(true)) => LibraryStatus::Online,
        _ => LibraryStatus::Offline,
    }
}

/// Handle to a Printdex engine. Cheap to clone; clones share everything.
#[derive(Clone, Debug)]
pub struct Engine {
    pub(crate) db: Db,
    pub(crate) events: broadcast::Sender<EngineEvent>,
    scanning: Arc<Mutex<HashSet<LibraryId>>>,
    thumbs: ThumbnailService,
    pub(crate) sidecars: SidecarWriter,
    pub(crate) install: crate::ids::Uuid,
    pub(crate) fs: Arc<dyn Fs>,
    pub(crate) data_dir: PathBuf,
    /// Models whose file action a crash interrupted and opening finished.
    recovered: Arc<Mutex<Vec<String>>>,
    /// Running backup jobs and the open repository.
    pub(crate) backup: Arc<crate::backup::run::BackupState>,
    pub(crate) cache_dir: PathBuf,
    /// Backup secrets (session and OS credential store).
    pub(crate) secrets: Arc<crate::backup::secrets::Store>,
}

/// Removes a library from the "scanning" set on every exit path. Holds the
/// library's lock file, so another Printdex process (the CLI next to the
/// app) cannot scan it at the same time; the OS drops the lock if the
/// process dies.
pub(crate) struct ScanGuard<'a> {
    scanning: &'a Mutex<HashSet<LibraryId>>,
    id: LibraryId,
    _lock: std::fs::File,
}

/// Takes the cross-process scan lock for library `id`.
fn scan_lock(data_dir: &Path, id: LibraryId) -> Result<std::fs::File> {
    let dir = data_dir.join("locks");
    std::fs::create_dir_all(&dir).map_err(|source| CoreError::Io {
        path: dir.clone(),
        source,
    })?;
    let path = dir.join(format!("scan-{id}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|source| CoreError::Io {
            path: path.clone(),
            source,
        })?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(CoreError::AlreadyScanning(id)),
        Err(std::fs::TryLockError::Error(source)) => Err(CoreError::Io { path, source }),
    }
}

impl Drop for ScanGuard<'_> {
    fn drop(&mut self) {
        lock(self.scanning).remove(&self.id);
    }
}

fn lock(m: &Mutex<HashSet<LibraryId>>) -> std::sync::MutexGuard<'_, HashSet<LibraryId>> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Engine {
    /// Opens the engine with its database at `data_dir/printdex.db` and its
    /// caches in `data_dir/cache`, creating folders and database if needed.
    pub async fn open(data_dir: &Path) -> Result<Engine> {
        Engine::open_with_cache(data_dir, &data_dir.join("cache")).await
    }

    /// Like [`Engine::open`], with caches (thumbnails) in `cache_dir`.
    /// Must be called inside a tokio runtime: it starts the thumbnail
    /// workers.
    pub async fn open_with_cache(data_dir: &Path, cache_dir: &Path) -> Result<Engine> {
        Engine::open_with_fs(data_dir, cache_dir, Arc::new(RealFs)).await
    }

    /// Like [`Engine::open_with_cache`], scanning through `fs` (tests and
    /// the bench pass a [`crate::fs::ChaosFs`]).
    pub async fn open_with_fs(
        data_dir: &Path,
        cache_dir: &Path,
        fs: Arc<dyn Fs>,
    ) -> Result<Engine> {
        std::fs::create_dir_all(data_dir).map_err(|source| CoreError::Io {
            path: data_dir.to_path_buf(),
            source,
        })?;
        crate::backup::restore::apply_staged(data_dir)?;
        let db = Db::open(&data_dir.join("printdex.db")).await?;
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let thumbs = ThumbnailService::start(db.clone(), events.clone(), cache_dir);
        let install = db.install_id().await?;
        let sidecars = SidecarWriter::start(db.clone(), install, crate::sidecar::DEBOUNCE);
        let engine = Engine {
            db,
            events,
            scanning: Arc::new(Mutex::new(HashSet::new())),
            thumbs,
            sidecars,
            install,
            fs,
            data_dir: data_dir.to_path_buf(),
            recovered: Arc::new(Mutex::new(Vec::new())),
            backup: Arc::new(crate::backup::run::BackupState::default()),
            cache_dir: cache_dir.to_path_buf(),
            secrets: Arc::new(crate::backup::secrets::Store::default()),
        };
        match engine.reconcile_ops().await {
            Ok(names) => *engine.recovered.lock().unwrap_or_else(|p| p.into_inner()) = names,
            Err(e) => tracing::warn!(error = %e, "could not check interrupted file actions"),
        }
        Ok(engine)
    }

    /// This install's id (the host name of its backups).
    pub fn install_id(&self) -> crate::ids::Uuid {
        self.install
    }

    /// Names of models whose rename, move or trash a crash interrupted and
    /// opening finished. Returns them once.
    pub fn take_recovered(&self) -> Vec<String> {
        std::mem::take(&mut *self.recovered.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// The per-user data folder for Printdex on this OS.
    pub fn default_data_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "Printdex").map(|d| d.data_dir().to_path_buf())
    }

    /// The per-user cache folder for Printdex on this OS.
    pub fn default_cache_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "Printdex").map(|d| d.cache_dir().to_path_buf())
    }

    /// The thumbnail queue.
    pub fn thumbnails(&self) -> &ThumbnailService {
        &self.thumbs
    }

    /// Subscribes to engine events from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    /// Registers a library folder. The path must be an existing directory
    /// with a UTF-8 path that neither contains nor lies inside another library.
    pub async fn add_library(&self, path: &Path, display_name: Option<String>) -> Result<Library> {
        let path = match dunce::canonicalize(path) {
            Ok(p) => p,
            // A symlink (or mount point) whose target is unreachable is an
            // offline share, not a bad path.
            Err(_) if std::fs::symlink_metadata(path).is_ok() => {
                return Err(CoreError::LibraryOffline(path.to_path_buf()));
            }
            Err(_) => return Err(CoreError::LibraryPathInvalid(path.to_path_buf())),
        };
        if !path.is_dir() || path.to_str().is_none() {
            return Err(CoreError::LibraryPathInvalid(path));
        }
        for existing in self.db.libraries().await? {
            if path.starts_with(&existing.path) || existing.path.starts_with(&path) {
                return Err(CoreError::LibraryOverlaps {
                    existing: existing.path,
                });
            }
        }
        let display_name = display_name
            .filter(|n| !n.trim().is_empty())
            .or_else(|| path.file_name().and_then(|n| n.to_str()).map(str::to_owned))
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let probe = path.clone();
        let kind = tokio::task::spawn_blocking(move || crate::volume::detect(&probe))
            .await
            .unwrap_or(LibraryKind::Local);
        let lib = Library {
            id: LibraryId::new(),
            path,
            display_name,
            kind,
            change_mode: ChangeMode::Auto,
            sidecar_enabled: true,
            last_full_scan_at: None,
        };
        self.db.insert_library(&lib).await?;
        let _ = self.events.send(EngineEvent::LibraryAdded(lib.clone()));
        Ok(lib)
    }

    /// All libraries, oldest first.
    pub async fn libraries(&self) -> Result<Vec<Library>> {
        self.db.libraries().await
    }

    /// The library registered at `path`, if any.
    pub async fn library_by_path(&self, path: &Path) -> Result<Option<Library>> {
        let path = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        Ok(self
            .db
            .libraries()
            .await?
            .into_iter()
            .find(|l| l.path == path))
    }

    /// Scans library `id`. Only one scan per library runs at a time.
    pub async fn rescan(&self, id: LibraryId, cancel: CancellationToken) -> Result<ScanSummary> {
        self.scan(id, cancel, None).await
    }

    /// Scans only `paths` (folders or files relative to library `id`'s
    /// root) of library `id`; the rest of the library keeps what the
    /// database knows. Used after the watcher saw changes.
    pub async fn rescan_paths(
        &self,
        id: LibraryId,
        paths: Vec<String>,
        cancel: CancellationToken,
    ) -> Result<ScanSummary> {
        if paths.is_empty() {
            return Ok(ScanSummary {
                partial: true,
                ..ScanSummary::default()
            });
        }
        self.scan(id, cancel, Some(paths)).await
    }

    async fn scan(
        &self,
        id: LibraryId,
        cancel: CancellationToken,
        subtrees: Option<Vec<String>>,
    ) -> Result<ScanSummary> {
        let _guard = self.claim(id)?;
        let lib = self.db.library(id).await?;
        let ctx = SidecarCtx {
            writer: &self.sidecars,
            install: self.install,
        };
        match subtrees {
            None => {
                scan_library(
                    self.fs.clone(),
                    &self.db,
                    &lib,
                    &self.events,
                    cancel,
                    Some(ctx),
                )
                .await
            }
            Some(paths) => {
                scan_subtrees(
                    self.fs.clone(),
                    &self.db,
                    &lib,
                    &self.events,
                    cancel,
                    Some(ctx),
                    &paths,
                )
                .await
            }
        }
    }

    /// Takes library `id` for a scan or a file action: no other scan or
    /// file action of it runs, in this process or another, until the
    /// guard drops.
    pub(crate) fn claim(&self, id: LibraryId) -> Result<ScanGuard<'_>> {
        if !lock(&self.scanning).insert(id) {
            return Err(CoreError::AlreadyScanning(id));
        }
        let file_lock = match scan_lock(&self.data_dir, id) {
            Ok(f) => f,
            Err(e) => {
                lock(&self.scanning).remove(&id);
                return Err(e);
            }
        };
        Ok(ScanGuard {
            scanning: &self.scanning,
            id,
            _lock: file_lock,
        })
    }

    /// Sets how library `id` notices changes; a running monitor picks it
    /// up at once.
    pub async fn set_change_mode(&self, id: LibraryId, mode: ChangeMode) -> Result<Library> {
        self.db.set_change_mode(id, mode).await?;
        let lib = self.db.library(id).await?;
        let _ = self.events.send(EngineEvent::LibraryChanged(lib.clone()));
        Ok(lib)
    }

    /// Searches models; see [`Db::search_models`].
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<ModelHit>> {
        self.db.search_models(query, limit).await
    }

    /// Searches models in `scope`, narrowed by `filter`, in `sort` order.
    pub async fn search_in(
        &self,
        scope: &Scope,
        query: &str,
        filter: &SearchFilter,
        sort: SortOrder,
        limit: u32,
    ) -> Result<Vec<ModelHit>> {
        self.db
            .search_models_in(scope, query, filter, sort, limit)
            .await
    }

    /// Number of models a search would return without a limit.
    pub async fn count(&self, scope: &Scope, query: &str, filter: &SearchFilter) -> Result<u64> {
        self.db.count_models(scope, query, filter).await
    }

    /// Model `id` with its present files (a missing model: the files it
    /// last had, and `missing_since` set); `None` when it was forgotten.
    pub async fn model(&self, id: ModelId) -> Result<Option<ModelDetail>> {
        self.db.model_detail(id).await
    }

    /// Removes library `id` and everything Printdex knows about it. Files
    /// on disk are never touched. Fails while the library is scanning.
    pub async fn remove_library(&self, id: LibraryId) -> Result<()> {
        if lock(&self.scanning).contains(&id) {
            return Err(CoreError::AlreadyScanning(id));
        }
        if !self.db.delete_library(id).await? {
            return Err(CoreError::LibraryNotFound(id));
        }
        let _ = self.events.send(EngineEvent::LibraryRemoved(id));
        Ok(())
    }

    /// Renames library `id`. Blank names are rejected.
    pub async fn rename_library(&self, id: LibraryId, name: &str) -> Result<Library> {
        let name = name.trim();
        if name.is_empty() {
            return Err(CoreError::LibraryNameInvalid);
        }
        self.db.rename_library(id, name).await?;
        let lib = self.db.library(id).await?;
        let _ = self.events.send(EngineEvent::LibraryRenamed(lib.clone()));
        Ok(lib)
    }

    /// Whether library `id`'s folder can be listed right now. Gives up
    /// after [`STATUS_TIMEOUT`] (a hung network share counts as offline).
    pub async fn library_status(&self, id: LibraryId) -> Result<LibraryStatus> {
        let lib = self.db.library(id).await?;
        Ok(folder_status(self.fs.clone(), lib.path).await)
    }

    /// Present-model counts per library; libraries without models are absent.
    pub async fn library_counts(&self) -> Result<HashMap<LibraryId, u64>> {
        self.db.library_model_counts().await
    }

    /// Model counts for the sidebar's smart filters.
    pub async fn smart_counts(&self) -> Result<SmartCounts> {
        self.db.smart_counts().await
    }

    /// Row counts.
    pub async fn stats(&self) -> Result<Stats> {
        self.db.stats().await
    }
}
