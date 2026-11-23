//! Where a sync space keeps its files: a folder (inside a library or
//! anywhere on disk), a remote target through OpenDAL, or memory (tests).
//!
//! Paths are `/`-separated and relative to the space's root. Every install
//! writes only its own files, so the stores need no locking; `write`
//! replaces a file atomically and `write_once` never replaces one.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use crate::backup::Target;
use crate::backup::TargetSecrets;
use crate::error::{Result, SyncError};

/// A boxed future returned by [`SpaceStore`] methods.
pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// The files of one sync space.
pub trait SpaceStore: Send + Sync + std::fmt::Debug + 'static {
    /// Names of the files (not folders) directly in `dir`; empty when `dir`
    /// does not exist.
    fn list<'a>(&'a self, dir: &'a str) -> StoreFuture<'a, Vec<String>>;
    /// Names of the folders directly in `dir`.
    fn list_dirs<'a>(&'a self, dir: &'a str) -> StoreFuture<'a, Vec<String>>;
    /// The file's bytes, `None` when it does not exist.
    fn read<'a>(&'a self, path: &'a str) -> StoreFuture<'a, Option<Vec<u8>>>;
    /// Writes (or replaces) a file atomically: readers see the old or the
    /// new bytes, never a mix.
    fn write<'a>(&'a self, path: &'a str, bytes: Vec<u8>) -> StoreFuture<'a, ()>;
    /// Writes a file only if it does not exist yet. `false` when it did.
    fn write_once<'a>(&'a self, path: &'a str, bytes: Vec<u8>) -> StoreFuture<'a, bool>;
    /// Removes a file; a missing file is fine.
    fn delete<'a>(&'a self, path: &'a str) -> StoreFuture<'a, ()>;
    /// Where the space is, for display.
    fn location(&self) -> String;
}

/// Turns an I/O error into the sync error the UI explains.
pub(crate) fn io_error(e: &std::io::Error) -> SyncError {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::StorageFull | K::QuotaExceeded => SyncError::Full,
        K::PermissionDenied | K::ReadOnlyFilesystem => SyncError::ReadOnly,
        K::NotFound | K::NotConnected | K::TimedOut | K::ConnectionRefused => {
            SyncError::Unreachable(e.to_string())
        }
        _ => SyncError::Storage(e.to_string()),
    }
}

/// Turns an OpenDAL error into a sync error.
pub(crate) fn dal_error(e: &opendal::Error) -> SyncError {
    use opendal::ErrorKind as K;
    match e.kind() {
        K::PermissionDenied => SyncError::Credentials,
        _ if e.is_temporary() => SyncError::Unreachable(e.to_string()),
        _ => SyncError::Storage(e.to_string()),
    }
}

/// A path inside the space, refusing anything that could leave it.
fn checked(path: &str) -> Result<&str> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|seg| seg.is_empty() || seg == "." || seg == "..");
    if bad {
        return Err(SyncError::Storage(format!("bad path in sync space: {path}")).into());
    }
    Ok(path)
}

fn dir_ok(dir: &str) -> Result<&str> {
    if dir.is_empty() {
        Ok(dir)
    } else {
        checked(dir)
    }
}

/// A space in a folder: `<library>/.printdex/sync` or any folder the user
/// picked (USB drive, mounted share, Dropbox/Nextcloud folder).
#[derive(Clone, Debug)]
pub struct FolderStore {
    root: PathBuf,
}

impl FolderStore {
    pub fn new(root: impl Into<PathBuf>) -> FolderStore {
        FolderStore { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn full(&self, rel: &str) -> PathBuf {
        let mut p = self.root.clone();
        for seg in rel.split('/').filter(|s| !s.is_empty()) {
            p.push(seg);
        }
        p
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T> {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(io_error(&e).into()),
        Err(e) => Err(SyncError::Storage(e.to_string()).into()),
    }
}

fn list_dir(dir: &Path, want_dirs: bool) -> std::io::Result<Vec<String>> {
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for entry in rd {
        let entry = entry?;
        let is_dir = entry.file_type()?.is_dir();
        if is_dir != want_dirs {
            continue;
        }
        if let Some(name) = entry.file_name().to_str() {
            out.push(name.to_owned());
        }
    }
    out.sort();
    Ok(out)
}

/// Writes `bytes` next to `dest` under a temporary name, syncs and renames.
fn write_atomic(dest: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    crate::fsx::durable_write(dest, bytes, crate::fsx::Durability::UserData)
}

impl SpaceStore for FolderStore {
    fn list<'a>(&'a self, dir: &'a str) -> StoreFuture<'a, Vec<String>> {
        Box::pin(async move {
            let full = self.full(dir_ok(dir)?);
            blocking(move || list_dir(&full, false)).await
        })
    }

    fn list_dirs<'a>(&'a self, dir: &'a str) -> StoreFuture<'a, Vec<String>> {
        Box::pin(async move {
            let full = self.full(dir_ok(dir)?);
            blocking(move || list_dir(&full, true)).await
        })
    }

    fn read<'a>(&'a self, path: &'a str) -> StoreFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            let full = self.full(checked(path)?);
            blocking(move || match std::fs::read(&full) {
                Ok(b) => Ok(Some(b)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            })
            .await
        })
    }

    fn write<'a>(&'a self, path: &'a str, bytes: Vec<u8>) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let full = self.full(checked(path)?);
            blocking(move || write_atomic(&full, &bytes)).await
        })
    }

    fn write_once<'a>(&'a self, path: &'a str, bytes: Vec<u8>) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let full = self.full(checked(path)?);
            blocking(move || {
                if full.exists() {
                    return Ok(false);
                }
                write_atomic(&full, &bytes)?;
                Ok(true)
            })
            .await
        })
    }

    fn delete<'a>(&'a self, path: &'a str) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let full = self.full(checked(path)?);
            // Safe because callers delete only batches a newer snapshot covers, or a forgotten device's files.
            blocking(move || match std::fs::remove_file(&full) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            })
            .await
        })
    }

    fn location(&self) -> String {
        self.root.display().to_string()
    }
}

/// A space on an S3, WebDAV or SFTP target.
#[derive(Clone, Debug)]
pub struct RemoteStore {
    op: opendal::Operator,
    location: String,
}

impl RemoteStore {
    pub fn new(op: opendal::Operator, location: String) -> RemoteStore {
        RemoteStore { op, location }
    }

    async fn names(&self, dir: &str, want_dirs: bool) -> Result<Vec<String>> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{}/", dir_ok(dir)?)
        };
        let entries = match self.op.list(&prefix).await {
            Ok(e) => e,
            Err(e) if e.kind() == opendal::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(dal_error(&e).into()),
        };
        let mut out: Vec<String> = entries
            .into_iter()
            .filter(|e| e.metadata().is_dir() == want_dirs)
            .filter_map(|e| {
                let rel = e.path().strip_prefix(&prefix).unwrap_or(e.path());
                let name = rel.trim_end_matches('/');
                (!name.is_empty() && !name.contains('/')).then(|| name.to_owned())
            })
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }
}

impl SpaceStore for RemoteStore {
    fn list<'a>(&'a self, dir: &'a str) -> StoreFuture<'a, Vec<String>> {
        Box::pin(self.names(dir, false))
    }

    fn list_dirs<'a>(&'a self, dir: &'a str) -> StoreFuture<'a, Vec<String>> {
        Box::pin(self.names(dir, true))
    }

    fn read<'a>(&'a self, path: &'a str) -> StoreFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move {
            match self.op.read(checked(path)?).await {
                Ok(b) => Ok(Some(b.to_vec())),
                Err(e) if e.kind() == opendal::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(dal_error(&e).into()),
            }
        })
    }

    fn write<'a>(&'a self, path: &'a str, bytes: Vec<u8>) -> StoreFuture<'a, ()> {
        // One PUT: object stores and WebDAV replace a file as a whole.
        Box::pin(async move {
            self.op
                .write(checked(path)?, bytes)
                .await
                .map(|_| ())
                .map_err(|e| dal_error(&e).into())
        })
    }

    fn write_once<'a>(&'a self, path: &'a str, bytes: Vec<u8>) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let path = checked(path)?;
            match self.op.exists(path).await {
                Ok(true) => return Ok(false),
                Ok(false) => {}
                Err(e) => return Err(dal_error(&e).into()),
            }
            self.op
                .write(path, bytes)
                .await
                .map(|_| true)
                .map_err(|e| dal_error(&e).into())
        })
    }

    fn delete<'a>(&'a self, path: &'a str) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            match self.op.delete(checked(path)?).await {
                Err(e) if e.kind() != opendal::ErrorKind::NotFound => Err(dal_error(&e).into()),
                _ => Ok(()),
            }
        })
    }

    fn location(&self) -> String {
        self.location.clone()
    }
}

/// A space in memory, shared by clones (tests and simulated installs).
#[derive(Clone, Debug, Default)]
pub struct MemStore {
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    fail_writes: Arc<Mutex<u32>>,
}

impl MemStore {
    pub fn new() -> MemStore {
        MemStore::default()
    }

    /// Makes the next `n` writes fail as if the target were unreachable.
    pub fn fail_next_writes(&self, n: u32) {
        *self.fail_writes.lock().unwrap_or_else(|p| p.into_inner()) = n;
    }

    /// Replaces a file's bytes directly (tests that damage files).
    pub fn put_raw(&self, path: &str, bytes: Vec<u8>) {
        self.lock().insert(path.to_owned(), bytes);
    }

    /// All file paths, sorted.
    pub fn paths(&self) -> Vec<String> {
        self.lock().keys().cloned().collect()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.files.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn check_write(&self) -> Result<()> {
        let mut n = self.fail_writes.lock().unwrap_or_else(|p| p.into_inner());
        if *n > 0 {
            *n -= 1;
            return Err(SyncError::Unreachable("simulated failure".into()).into());
        }
        Ok(())
    }

    fn children(&self, dir: &str, want_dirs: bool) -> Vec<String> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut out: Vec<String> = self
            .lock()
            .keys()
            .filter_map(|k| k.strip_prefix(&prefix))
            .filter_map(|rest| match rest.split_once('/') {
                Some((d, _)) if want_dirs => Some(d.to_owned()),
                None if !want_dirs => Some(rest.to_owned()),
                _ => None,
            })
            .collect();
        out.dedup();
        out
    }
}

impl SpaceStore for MemStore {
    fn list<'a>(&'a self, dir: &'a str) -> StoreFuture<'a, Vec<String>> {
        Box::pin(async move { Ok(self.children(dir_ok(dir)?, false)) })
    }

    fn list_dirs<'a>(&'a self, dir: &'a str) -> StoreFuture<'a, Vec<String>> {
        Box::pin(async move { Ok(self.children(dir_ok(dir)?, true)) })
    }

    fn read<'a>(&'a self, path: &'a str) -> StoreFuture<'a, Option<Vec<u8>>> {
        Box::pin(async move { Ok(self.lock().get(checked(path)?).cloned()) })
    }

    fn write<'a>(&'a self, path: &'a str, bytes: Vec<u8>) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            let path = checked(path)?;
            self.check_write()?;
            self.lock().insert(path.to_owned(), bytes);
            Ok(())
        })
    }

    fn write_once<'a>(&'a self, path: &'a str, bytes: Vec<u8>) -> StoreFuture<'a, bool> {
        Box::pin(async move {
            let path = checked(path)?;
            self.check_write()?;
            let mut files = self.lock();
            if files.contains_key(path) {
                return Ok(false);
            }
            files.insert(path.to_owned(), bytes);
            Ok(true)
        })
    }

    fn delete<'a>(&'a self, path: &'a str) -> StoreFuture<'a, ()> {
        Box::pin(async move {
            self.lock().remove(checked(path)?);
            Ok(())
        })
    }

    fn location(&self) -> String {
        "memory".into()
    }
}

/// The store for a target space.
pub fn open_store(
    target: &Target,
    secrets: &TargetSecrets,
    throttle_kib: Option<u32>,
) -> Result<Arc<dyn SpaceStore>> {
    if let Target::Folder { path } = target {
        return Ok(Arc::new(FolderStore::new(path.clone())));
    }
    let throttle = throttle_kib.map(|k| crate::backup::remote::Throttle(k.saturating_mul(1024)));
    let (op, location) = crate::backup::repo::operator(target, secrets, throttle)?;
    Ok(Arc::new(RemoteStore::new(op, location)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn folder_write_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let s = FolderStore::new(dir.path());
        rt().block_on(async {
            s.write("installs/a/log/1.chg", b"x".to_vec())
                .await
                .unwrap();
            s.write("installs/a/log/1.chg", b"y".to_vec())
                .await
                .unwrap();
            assert_eq!(s.list("installs/a/log").await.unwrap(), vec!["1.chg"]);
            assert_eq!(s.list_dirs("installs").await.unwrap(), vec!["a"]);
            assert_eq!(
                s.read("installs/a/log/1.chg").await.unwrap(),
                Some(b"y".to_vec())
            );
        });
    }

    #[test]
    fn write_once_keeps_first() {
        let dir = tempfile::tempdir().unwrap();
        let stores: Vec<Box<dyn SpaceStore>> = vec![
            Box::new(FolderStore::new(dir.path())),
            Box::new(MemStore::new()),
        ];
        rt().block_on(async {
            for s in stores {
                assert!(s.write_once("space.toml", b"one".to_vec()).await.unwrap());
                assert!(!s.write_once("space.toml", b"two".to_vec()).await.unwrap());
                assert_eq!(s.read("space.toml").await.unwrap(), Some(b"one".to_vec()));
            }
        });
    }

    #[test]
    fn list_missing_dir_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let s = FolderStore::new(dir.path().join("nope"));
        rt().block_on(async {
            assert!(s.list("installs").await.unwrap().is_empty());
            assert!(s.read("space.toml").await.unwrap().is_none());
            s.delete("space.toml").await.unwrap();
        });
    }

    #[test]
    fn paths_cannot_escape() {
        let s = MemStore::new();
        rt().block_on(async {
            for bad in ["../x", "/x", "a//b", "a/./b", "a\\b"] {
                assert!(s.write(bad, vec![]).await.is_err(), "{bad}");
            }
        });
    }

    #[test]
    fn mem_store_failures() {
        let s = MemStore::new();
        rt().block_on(async {
            s.fail_next_writes(1);
            assert!(s.write("a", b"1".to_vec()).await.is_err());
            assert!(s.read("a").await.unwrap().is_none());
            s.write("a", b"1".to_vec()).await.unwrap();
            assert_eq!(s.list("").await.unwrap(), vec!["a"]);
        });
    }
}
