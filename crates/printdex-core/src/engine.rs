//! The engine handle: the one entry point for the app and the CLI.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::db::{Db, Library, LibraryKind, ModelDetail, ModelHit, SearchFilter, SortOrder, Stats};
use crate::error::{CoreError, Result};
use crate::events::EngineEvent;
use crate::ids::{LibraryId, ModelId};
use crate::scan::{ScanSummary, scan_library};

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

async fn folder_status(path: PathBuf) -> LibraryStatus {
    let check = tokio::task::spawn_blocking(move || {
        std::fs::read_dir(&path).is_ok_and(|mut d| d.next().is_none_or(|e| e.is_ok()))
    });
    match tokio::time::timeout(STATUS_TIMEOUT, check).await {
        Ok(Ok(true)) => LibraryStatus::Online,
        _ => LibraryStatus::Offline,
    }
}

/// Handle to a Printdex engine. Cheap to clone; clones share everything.
#[derive(Clone, Debug)]
pub struct Engine {
    db: Db,
    events: broadcast::Sender<EngineEvent>,
    scanning: Arc<Mutex<HashSet<LibraryId>>>,
}

/// Removes a library from the "scanning" set on every exit path.
struct ScanGuard<'a> {
    scanning: &'a Mutex<HashSet<LibraryId>>,
    id: LibraryId,
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
    /// Opens the engine with its database at `data_dir/printdex.db`,
    /// creating the folder and database if needed.
    pub async fn open(data_dir: &Path) -> Result<Engine> {
        std::fs::create_dir_all(data_dir).map_err(|source| CoreError::Io {
            path: data_dir.to_path_buf(),
            source,
        })?;
        let db = Db::open(&data_dir.join("printdex.db")).await?;
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Ok(Engine {
            db,
            events,
            scanning: Arc::new(Mutex::new(HashSet::new())),
        })
    }

    /// The per-user data folder for Printdex on this OS.
    pub fn default_data_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "Printdex").map(|d| d.data_dir().to_path_buf())
    }

    /// Subscribes to engine events from now on.
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    /// Registers a library folder. The path must be an existing directory
    /// with a UTF-8 path that neither contains nor lies inside another library.
    pub async fn add_library(&self, path: &Path, display_name: Option<String>) -> Result<Library> {
        let path = dunce::canonicalize(path)
            .map_err(|_| CoreError::LibraryPathInvalid(path.to_path_buf()))?;
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
        let lib = Library {
            id: LibraryId::new(),
            path,
            display_name,
            kind: LibraryKind::Local,
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
        if !lock(&self.scanning).insert(id) {
            return Err(CoreError::AlreadyScanning(id));
        }
        let _guard = ScanGuard {
            scanning: &self.scanning,
            id,
        };
        let lib = self.db.library(id).await?;
        scan_library(&self.db, &lib, &self.events, cancel).await
    }

    /// Searches models; see [`Db::search_models`].
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<ModelHit>> {
        self.db.search_models(query, limit).await
    }

    /// Searches models within one library (or all when `library` is
    /// `None`), narrowed by `filter`, in `sort` order.
    pub async fn search_in(
        &self,
        library: Option<LibraryId>,
        query: &str,
        filter: &SearchFilter,
        sort: SortOrder,
        limit: u32,
    ) -> Result<Vec<ModelHit>> {
        self.db
            .search_models_in(library, query, filter, sort, limit)
            .await
    }

    /// Number of models a search would return without a limit.
    pub async fn count(
        &self,
        library: Option<LibraryId>,
        query: &str,
        filter: &SearchFilter,
    ) -> Result<u64> {
        self.db.count_models(library, query, filter).await
    }

    /// Model `id` with its present files; `None` when it is gone.
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
        Ok(folder_status(lib.path).await)
    }

    /// Present-model counts per library; libraries without models are absent.
    pub async fn library_counts(&self) -> Result<HashMap<LibraryId, u64>> {
        self.db.library_model_counts().await
    }

    /// Row counts.
    pub async fn stats(&self) -> Result<Stats> {
        self.db.stats().await
    }
}
