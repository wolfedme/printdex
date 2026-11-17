//! The filesystem as the scanner sees it. Production uses [`RealFs`]; tests
//! and the bench use [`ChaosFs`] to add latency, failures and vanishing
//! mounts, the way a slow or flaky NAS behaves.

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::time::system_time_to_ns;

/// What a directory entry is, without following symlinks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// One entry of a directory listing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: OsString,
    pub kind: EntryKind,
}

/// Size and modification time of a file (symlinks followed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Meta {
    pub len: u64,
    pub mtime_ns: i64,
    pub is_dir: bool,
}

/// A readable, seekable file.
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

/// The filesystem calls the scanner makes. Blocking: call from the blocking
/// or CPU pool.
pub trait Fs: Send + Sync + fmt::Debug + 'static {
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>>;
    fn metadata(&self, path: &Path) -> io::Result<Meta>;
    fn open(&self, path: &Path) -> io::Result<Box<dyn ReadSeek>>;
}

/// The operating system's filesystem.
#[derive(Clone, Copy, Debug, Default)]
pub struct RealFs;

impl Fs for RealFs {
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let kind = match entry.file_type() {
                Ok(t) if t.is_symlink() => EntryKind::Symlink,
                Ok(t) if t.is_dir() => EntryKind::Dir,
                Ok(t) if t.is_file() => EntryKind::File,
                Ok(_) => EntryKind::Other,
                Err(e) => return Err(e),
            };
            out.push(DirEntry {
                name: entry.file_name(),
                kind,
            });
        }
        Ok(out)
    }

    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        let m = std::fs::metadata(path)?;
        Ok(Meta {
            len: m.len(),
            mtime_ns: m.modified().map(system_time_to_ns).unwrap_or(0),
            is_dir: m.is_dir(),
        })
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn ReadSeek>> {
        Ok(Box::new(std::fs::File::open(path)?))
    }
}

/// A filesystem that behaves like a bad network share: every call waits
/// `latency`, a share of calls fail, and subtrees can be "unmounted".
#[derive(Debug, Default)]
pub struct ChaosFs {
    latency: Duration,
    /// Fail one call in `fail_every` (0: never).
    fail_every: u64,
    calls: AtomicU64,
    unmounted: Mutex<Vec<PathBuf>>,
}

impl ChaosFs {
    pub fn new() -> ChaosFs {
        ChaosFs::default()
    }

    /// Every call waits this long first.
    pub fn with_latency(mut self, latency: Duration) -> ChaosFs {
        self.latency = latency;
        self
    }

    /// Every `n`th call fails with a timeout (0 turns failures off).
    pub fn failing_every(mut self, n: u64) -> ChaosFs {
        self.fail_every = n;
        self
    }

    /// Calls under `path` fail as if the share dropped.
    pub fn unmount(&self, path: &Path) {
        self.unmounted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(path.to_path_buf());
    }

    /// Undoes [`ChaosFs::unmount`].
    pub fn remount(&self, path: &Path) {
        self.unmounted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|p| p != path);
    }

    /// Filesystem calls made so far.
    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }

    fn check(&self, path: &Path) -> io::Result<()> {
        let n = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
        if !self.latency.is_zero() {
            std::thread::sleep(self.latency);
        }
        let gone = self
            .unmounted
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .any(|u| path.starts_with(u));
        if gone {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "share unmounted (chaos)",
            ));
        }
        if self.fail_every > 0 && n.is_multiple_of(self.fail_every) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "share timed out (chaos)",
            ));
        }
        Ok(())
    }
}

impl Fs for ChaosFs {
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        self.check(path)?;
        RealFs.read_dir(path)
    }

    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        self.check(path)?;
        RealFs.metadata(path)
    }

    fn open(&self, path: &Path) -> io::Result<Box<dyn ReadSeek>> {
        self.check(path)?;
        RealFs.open(path)
    }
}

impl<T: Fs> Fs for std::sync::Arc<T> {
    fn read_dir(&self, path: &Path) -> io::Result<Vec<DirEntry>> {
        (**self).read_dir(path)
    }
    fn metadata(&self, path: &Path) -> io::Result<Meta> {
        (**self).metadata(path)
    }
    fn open(&self, path: &Path) -> io::Result<Box<dyn ReadSeek>> {
        (**self).open(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmounted_subtree_fails_until_remounted() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("a")).unwrap();
        let fs = ChaosFs::new();
        assert!(fs.read_dir(&d.path().join("a")).is_ok());
        fs.unmount(d.path());
        assert!(fs.read_dir(&d.path().join("a")).is_err());
        fs.remount(d.path());
        assert!(fs.metadata(&d.path().join("a")).unwrap().is_dir);
    }

    #[test]
    fn fails_every_nth_call() {
        let d = tempfile::tempdir().unwrap();
        let fs = ChaosFs::new().failing_every(3);
        let r: Vec<bool> = (0..6).map(|_| fs.metadata(d.path()).is_ok()).collect();
        assert_eq!(r, [true, true, false, true, true, false]);
        assert_eq!(fs.calls(), 6);
    }

    #[test]
    fn real_fs_lists_kinds() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("dir")).unwrap();
        std::fs::write(d.path().join("f"), b"x").unwrap();
        let mut e = RealFs.read_dir(d.path()).unwrap();
        e.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(e[0].kind, EntryKind::Dir);
        assert_eq!(e[1].kind, EntryKind::File);
    }
}
