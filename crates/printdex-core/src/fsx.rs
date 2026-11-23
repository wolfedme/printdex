//! The one place for file writes and moves that must not lose data:
//! durable writes, a rename that never replaces, verified tree copies and
//! sweeping of leftover temporary files.
//!
//! Everything here is blocking (`std::fs`); async callers wrap it in
//! `spawn_blocking`. Every function calls [`faults::check`] before each real
//! operation, so tests can make the n-th one fail or change something just
//! before it runs (see [`faults`]).

pub mod faults;

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use xxhash_rust::xxh3::Xxh3;

use self::faults::Op;

/// How hard [`durable_write`] works to make the new contents survive a
/// power cut.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Durability {
    /// Data the user would miss (sidecars, settings, the database's
    /// companions): fsyncs the temporary file before the rename and, on
    /// Unix, the parent folder after it.
    UserData,
    /// Data that can be rebuilt (thumbnails, caches): no fsyncs. The rename
    /// still means readers see the old or the new contents, never a mix.
    Cache,
}

/// Replaces `path` with `bytes` atomically: writes a temporary file named
/// `.<name>.tmp-<uuid v7 simple>` in the same folder, then renames it over
/// `path`. Readers see the old or the new contents, never a partial file.
///
/// Never creates folders: if the parent folder is missing this returns
/// `NotFound`. On any error the temporary file is removed and `path` is
/// left as it was. With [`Durability::UserData`] it fsyncs the temporary
/// file and, on Unix, the folder (filesystems that can't fsync a folder,
/// like some network mounts, are tolerated).
///
/// The new file gets default permissions, not the old file's.
///
/// Fault injection: [`Op::Write`] and then [`Op::Rename`], both with `path`.
pub fn durable_write(path: &Path, bytes: &[u8], d: Durability) -> io::Result<()> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("no file name in {}", path.display()),
        )
    })?;
    let dir = parent_dir(path);
    let mut tmp_name = OsString::from(".");
    tmp_name.push(name);
    tmp_name.push(format!(".tmp-{}", uuid::Uuid::now_v7().simple()));
    let tmp = dir.join(tmp_name);

    let file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    if let Err(e) = write_then_rename(file, &tmp, path, bytes, d) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if d == Durability::UserData {
        sync_dir(dir)?;
    }
    Ok(())
}

fn write_then_rename(
    mut file: File,
    tmp: &Path,
    path: &Path,
    bytes: &[u8],
    d: Durability,
) -> io::Result<()> {
    faults::check(Op::Write, path)?;
    file.write_all(bytes)?;
    if d == Durability::UserData {
        file.sync_all()?;
    }
    // Windows can't rename a file that is still open.
    drop(file);
    faults::check(Op::Rename, path)?;
    fs::rename(tmp, path)
}

/// The folder holding `path`; `.` for a bare file name.
fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// Fsyncs a folder so a rename in it survives a power cut. Filesystems that
/// refuse (`EINVAL` on some network and FUSE mounts) are tolerated.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    match File::open(dir).and_then(|f| f.sync_all()) {
        Err(e) if matches!(e.kind(), ErrorKind::InvalidInput | ErrorKind::Unsupported) => Ok(()),
        r => r,
    }
}

/// Windows has no folder fsync; NTFS journals the rename itself.
#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Renames `from` to `to` (a file or a folder), but never replaces
/// anything: returns `ErrorKind::AlreadyExists`, with both left untouched,
/// if `to` exists.
///
/// - Linux and macOS: `renameat2(RENAME_NOREPLACE)` / `renameatx_np(RENAME_EXCL)`
///   via `rustix::fs::renameat_with`, which is atomic.
/// - Windows: a file is hard-linked to `to` and then `from` is removed; a
///   folder goes through `std::fs::rename`, which Windows refuses to let
///   replace a folder, after checking that `to` is missing.
/// - Where the filesystem doesn't support that (`EINVAL`, `ENOSYS`,
///   `EOPNOTSUPP` on Unix; `Unsupported` or `PermissionDenied` from
///   `hard_link` on FAT or SMB) and on other platforms, it falls back to:
///   check `to` is missing, `std::fs::rename`, then check `from` is gone and
///   `to` exists. That fallback has a small race window between the check
///   and the rename.
///
/// On a case-insensitive filesystem a rename that only changes case is
/// refused, because `to` already "exists".
///
/// Fault injection: [`Op::Rename`] with `to`.
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    faults::check(Op::Rename, to)?;
    rename_no_replace_imp(from, to)
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn rename_no_replace_imp(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;

    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(e) if e == Errno::EXIST => Err(already_exists(to)),
        Err(e)
            if e == Errno::INVAL
                || e == Errno::NOSYS
                || e == Errno::OPNOTSUPP
                || e == Errno::NOTSUP =>
        {
            rename_fallback(from, to)
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(windows)]
fn rename_no_replace_imp(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(from)?.is_dir() {
        if exists_no_follow(to)? {
            return Err(already_exists(to));
        }
        return fs::rename(from, to);
    }
    match fs::hard_link(from, to) {
        Ok(()) => {
            if let Err(e) = fs::remove_file(from) {
                // Undo, so the file isn't left in two places.
                let _ = fs::remove_file(to);
                return Err(e);
            }
            Ok(())
        }
        // ERROR_INVALID_FUNCTION (1) and ERROR_NOT_SUPPORTED (50): FAT, SMB.
        Err(e)
            if matches!(
                e.kind(),
                ErrorKind::Unsupported | ErrorKind::PermissionDenied
            ) || matches!(e.raw_os_error(), Some(1 | 50)) =>
        {
            rename_fallback(from, to)
        }
        Err(e) => Err(e),
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    windows
)))]
fn rename_no_replace_imp(from: &Path, to: &Path) -> io::Result<()> {
    rename_fallback(from, to)
}

/// Check, rename, verify: for filesystems without an atomic no-replace
/// rename. Racy between the check and the rename.
fn rename_fallback(from: &Path, to: &Path) -> io::Result<()> {
    if exists_no_follow(to)? {
        return Err(already_exists(to));
    }
    fs::rename(from, to)?;
    if exists_no_follow(from)? || !exists_no_follow(to)? {
        return Err(io::Error::other(format!(
            "renaming {} to {} did not move it",
            from.display(),
            to.display()
        )));
    }
    Ok(())
}

fn exists_no_follow(p: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(p) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn already_exists(to: &Path) -> io::Error {
    io::Error::new(
        ErrorKind::AlreadyExists,
        format!("{} already exists", to.display()),
    )
}

/// One entry of a tree to copy, relative to its root (empty for the root).
enum Entry {
    Dir(PathBuf),
    File(PathBuf),
}

/// Copies the file or folder tree `from` to `to`, which must not exist
/// (`AlreadyExists`) and whose parent must (`NotFound`; no folders above
/// `to` are created).
///
/// Refuses up front, with `ErrorKind::Unsupported` and the message
/// `"contains links or special files"`, if the tree holds a symlink (or, on
/// Windows, a junction) or anything that is neither a file nor a folder.
/// Each copied file is fsynced. After everything is copied, every file's
/// size and xxh3-64 are checked in both the source and the copy against
/// what was copied, so a source that changed during the copy is caught
/// (`InvalidData`). On any failure `to` is removed; `from` is never
/// changed. Permissions and timestamps are not copied.
///
/// Fault injection: [`Op::Copy`] before each file, with its destination
/// path. Files are copied in sorted, depth-first order.
pub fn copy_tree_verified(from: &Path, to: &Path) -> io::Result<()> {
    let entries = scan_tree(from)?;
    // Creating the root also proves `to` didn't exist, so the cleanup below
    // only ever removes what this call made.
    match entries.first() {
        Some(Entry::Dir(_)) => fs::create_dir(to)?,
        _ => {
            if exists_no_follow(to)? {
                return Err(already_exists(to));
            }
        }
    }
    match copy_and_verify(from, to, &entries) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = match entries.first() {
                Some(Entry::Dir(_)) => fs::remove_dir_all(to),
                _ => fs::remove_file(to),
            };
            Err(e)
        }
    }
}

/// Lists the tree under `from` root first, sorted, refusing links and
/// special files.
fn scan_tree(from: &Path) -> io::Result<Vec<Entry>> {
    let refuse = || io::Error::new(ErrorKind::Unsupported, "contains links or special files");
    let root = fs::symlink_metadata(from)?;
    if root.is_file() {
        return Ok(vec![Entry::File(PathBuf::new())]);
    }
    if !root.is_dir() {
        return Err(refuse());
    }
    let mut out = vec![Entry::Dir(PathBuf::new())];
    let mut i = 0;
    while i < out.len() {
        if let Entry::Dir(rel) = &out[i] {
            let rel = rel.clone();
            let mut children = Vec::new();
            for e in fs::read_dir(from.join(&rel))? {
                let e = e?;
                // `file_type` doesn't follow symlinks.
                let t = e.file_type()?;
                let child = rel.join(e.file_name());
                if t.is_dir() {
                    children.push(Entry::Dir(child));
                } else if t.is_file() {
                    children.push(Entry::File(child));
                } else {
                    return Err(refuse());
                }
            }
            children.sort_by(|a, b| entry_rel(a).cmp(entry_rel(b)));
            // Depth-first: this folder's children come right after it.
            out.splice(i + 1..i + 1, children);
        }
        i += 1;
    }
    Ok(out)
}

fn entry_rel(e: &Entry) -> &Path {
    match e {
        Entry::Dir(p) | Entry::File(p) => p,
    }
}

/// Joins `rel` onto `root`, without the trailing separator an empty `rel`
/// would add.
fn under(root: &Path, rel: &Path) -> PathBuf {
    if rel.as_os_str().is_empty() {
        root.to_path_buf()
    } else {
        root.join(rel)
    }
}

fn copy_and_verify(from: &Path, to: &Path, entries: &[Entry]) -> io::Result<()> {
    let mut copied = Vec::new();
    for e in entries {
        match e {
            // The root was created by the caller.
            Entry::Dir(rel) if rel.as_os_str().is_empty() => {}
            Entry::Dir(rel) => fs::create_dir(to.join(rel))?,
            Entry::File(rel) => {
                let dst = under(to, rel);
                faults::check(Op::Copy, &dst)?;
                let sum = copy_file_hashed(&under(from, rel), &dst)?;
                copied.push((rel, sum));
            }
        }
    }
    for (rel, sum) in copied {
        if hash_file(&under(to, rel))? != sum || hash_file(&under(from, rel))? != sum {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                format!(
                    "the copy of {} does not match its source",
                    under(from, rel).display()
                ),
            ));
        }
    }
    for e in entries {
        if let Entry::Dir(rel) = e {
            let _ = sync_dir(&under(to, rel));
        }
    }
    let _ = sync_dir(parent_dir(to));
    Ok(())
}

/// Size and xxh3-64 of a file's contents.
type Sum = (u64, u64);

/// Copies `src` to a new file `dst`, fsyncs it, and returns the size and
/// hash of the bytes written.
fn copy_file_hashed(src: &Path, dst: &Path) -> io::Result<Sum> {
    let mut r = File::open(src)?;
    let mut w = OpenOptions::new().write(true).create_new(true).open(dst)?;
    let sum = stream(&mut r, |chunk| w.write_all(chunk))?;
    w.sync_all()?;
    Ok(sum)
}

fn hash_file(p: &Path) -> io::Result<Sum> {
    stream(&mut File::open(p)?, |_| Ok(()))
}

/// Reads `r` to the end, hashing it and handing each chunk to `sink`.
fn stream(r: &mut File, mut sink: impl FnMut(&[u8]) -> io::Result<()>) -> io::Result<Sum> {
    let mut h = Xxh3::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut len = 0u64;
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        h.update(&buf[..n]);
        sink(&buf[..n])?;
        len += n as u64;
    }
    Ok((len, h.digest()))
}

/// Removes temporary files left behind by a crash: entries in `dir` (one
/// level deep, not recursive) named `.*.tmp-*` (from [`durable_write`]) or
/// `.printdex-*.tmp`, last modified more than `older_than` ago. A matching
/// folder is removed with its contents. Errors are logged and skipped; a
/// missing or unreadable `dir` does nothing.
///
/// Fault injection: [`Op::Remove`] with each entry's path; a fault skips
/// that entry.
pub fn sweep_temp_files(dir: &Path, older_than: Duration) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    // Nothing is older than a cutoff before the epoch.
    let Some(cutoff) = SystemTime::now().checked_sub(older_than) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        if !name.to_str().is_some_and(is_temp_name) {
            continue;
        }
        // `DirEntry::metadata` doesn't follow symlinks.
        let Ok(meta) = e.metadata() else { continue };
        if meta.modified().map_or(true, |m| m > cutoff) {
            continue;
        }
        let path = e.path();
        if faults::check(Op::Remove, &path).is_err() {
            continue;
        }
        let r = if meta.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        if let Err(err) = r {
            tracing::debug!(path = %path.display(), %err, "could not sweep temporary file");
        }
    }
}

/// `.*.tmp-*` or `.printdex-*.tmp`.
fn is_temp_name(n: &str) -> bool {
    let Some(rest) = n.strip_prefix('.') else {
        return false;
    };
    rest.contains(".tmp-") || (rest.starts_with("printdex-") && rest.ends_with(".tmp"))
}

#[cfg(test)]
mod tests {
    use super::faults::{self, Action, Op};
    use super::*;
    use std::fs;
    use std::io::ErrorKind;
    use std::path::PathBuf;
    use std::time::SystemTime;

    /// Names in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn tree(root: &Path, files: &[(&str, &[u8])]) {
        for (rel, bytes) in files {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, bytes).unwrap();
        }
    }

    #[test]
    fn durable_write_replaces_and_leaves_no_temp() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.toml");
        fs::write(&p, b"old").unwrap();
        durable_write(&p, b"new contents", Durability::UserData).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"new contents");
        durable_write(&d.path().join("b.bin"), b"cache", Durability::Cache).unwrap();
        assert_eq!(fs::read(d.path().join("b.bin")).unwrap(), b"cache");
        assert_eq!(names(d.path()), ["a.toml", "b.bin"]);
    }

    #[test]
    fn durable_write_full_disk_keeps_original() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.toml");
        fs::write(&p, b"original").unwrap();
        let g = faults::arm(d.path(), Op::Write, 0, Action::Fail(ErrorKind::StorageFull));
        let err = durable_write(&p, b"replacement", Durability::UserData).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::StorageFull);
        assert_eq!(g.fired(), 1);
        assert_eq!(fs::read(&p).unwrap(), b"original");
        assert_eq!(names(d.path()), ["a.toml"]);
    }

    #[test]
    fn durable_write_never_creates_folders() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("missing").join("a.toml");
        let err = durable_write(&p, b"x", Durability::UserData).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(!d.path().join("missing").exists());
        assert!(names(d.path()).is_empty());
    }

    #[test]
    fn rename_no_replace_refuses_existing_file() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from.stl"), d.path().join("to.stl"));
        fs::write(&from, b"source").unwrap();
        fs::write(&to, b"target").unwrap();
        let err = rename_no_replace(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&from).unwrap(), b"source");
        assert_eq!(fs::read(&to).unwrap(), b"target");
    }

    #[test]
    fn rename_no_replace_refuses_existing_folder() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from"), d.path().join("to"));
        tree(&from, &[("a.stl", b"source")]);
        // An empty target folder: plain rename(2) would silently replace it.
        fs::create_dir(&to).unwrap();
        let err = rename_no_replace(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read(from.join("a.stl")).unwrap(), b"source");
        assert!(to.is_dir());
        assert!(names(&to).is_empty());
    }

    #[test]
    fn rename_no_replace_moves_when_free() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from.stl"), d.path().join("to.stl"));
        fs::write(&from, b"source").unwrap();
        rename_no_replace(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(fs::read(&to).unwrap(), b"source");

        let (dfrom, dto) = (d.path().join("dir"), d.path().join("moved"));
        tree(&dfrom, &[("x/y.3mf", b"y")]);
        rename_no_replace(&dfrom, &dto).unwrap();
        assert!(!dfrom.exists());
        assert_eq!(fs::read(dto.join("x/y.3mf")).unwrap(), b"y");
    }

    #[test]
    fn rename_fallback_refuses_existing_and_moves_when_free() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from"), d.path().join("to"));
        fs::write(&from, b"source").unwrap();
        fs::create_dir(&to).unwrap();
        let err = rename_fallback(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        fs::remove_dir(&to).unwrap();
        rename_fallback(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(fs::read(&to).unwrap(), b"source");
    }

    #[test]
    fn rename_no_replace_injected_fault_leaves_both() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from.stl"), d.path().join("to.stl"));
        fs::write(&from, b"source").unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(PathBuf::new()));
        let s = seen.clone();
        let g = faults::arm(
            d.path(),
            Op::Rename,
            0,
            Action::Before(Box::new(move |p| {
                *s.lock().unwrap() = p.to_path_buf();
                fs::write(p, b"raced in").unwrap();
            })),
        );
        let err = rename_no_replace(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(g.fired(), 1);
        assert_eq!(*seen.lock().unwrap(), to);
        assert_eq!(fs::read(&from).unwrap(), b"source");
        assert_eq!(fs::read(&to).unwrap(), b"raced in");
    }

    #[cfg(unix)]
    #[test]
    fn copy_tree_refuses_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from"), d.path().join("to"));
        tree(&from, &[("a.stl", b"a"), ("sub/b.stl", b"b")]);
        std::os::unix::fs::symlink(from.join("a.stl"), from.join("sub/link.stl")).unwrap();
        let err = copy_tree_verified(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Unsupported);
        assert_eq!(err.to_string(), "contains links or special files");
        assert!(!to.exists());
    }

    #[test]
    fn copy_tree_copies_everything() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from"), d.path().join("to"));
        tree(
            &from,
            &[
                ("a.stl", b"a"),
                ("sub/b.stl", b"bb"),
                ("sub/deep/c.3mf", b""),
            ],
        );
        fs::create_dir(from.join("empty")).unwrap();
        copy_tree_verified(&from, &to).unwrap();
        assert_eq!(fs::read(to.join("a.stl")).unwrap(), b"a");
        assert_eq!(fs::read(to.join("sub/b.stl")).unwrap(), b"bb");
        assert_eq!(fs::read(to.join("sub/deep/c.3mf")).unwrap(), b"");
        assert!(to.join("empty").is_dir());
        assert_eq!(fs::read(from.join("a.stl")).unwrap(), b"a");
    }

    #[test]
    fn copy_tree_refuses_existing_target() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from"), d.path().join("to"));
        tree(&from, &[("a.stl", b"a")]);
        tree(&to, &[("keep.stl", b"k")]);
        let err = copy_tree_verified(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read(to.join("keep.stl")).unwrap(), b"k");
    }

    #[test]
    fn copy_tree_detects_changed_content() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from"), d.path().join("to"));
        tree(&from, &[("a.stl", b"solid a"), ("b.stl", b"solid b")]);
        let a = from.join("a.stl");
        // Before the second file is copied, flip a byte in the first file's
        // source, which has already been copied.
        let g = faults::arm(
            &to,
            Op::Copy,
            1,
            Action::Before(Box::new(move |_| {
                let mut bytes = fs::read(&a).unwrap();
                bytes[0] ^= 0xff;
                fs::write(&a, bytes).unwrap();
            })),
        );
        let err = copy_tree_verified(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        assert_eq!(g.fired(), 1);
        assert!(!to.exists());
    }

    #[test]
    fn copy_tree_full_target_cleans_up() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from"), d.path().join("to"));
        let files: &[(&str, &[u8])] = &[("a.stl", b"a"), ("b.stl", b"b"), ("c/d.stl", b"d")];
        tree(&from, files);
        let g = faults::arm(&to, Op::Copy, 2, Action::Fail(ErrorKind::StorageFull));
        let err = copy_tree_verified(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::StorageFull);
        assert_eq!(g.fired(), 1);
        assert!(!to.exists());
        for (rel, bytes) in files {
            assert_eq!(&fs::read(from.join(rel)).unwrap(), bytes);
        }
    }

    #[test]
    fn sweep_removes_old_temp_only() {
        let d = tempfile::tempdir().unwrap();
        let old = SystemTime::now() - Duration::from_secs(2 * 3600);
        let entries = [
            ".a.toml.tmp-0190aaaa",
            ".printdex-00ff.tmp",
            ".b.json.tmp-0190bbbb",
            "keep.stl",
            ".hidden",
            "x.tmp-1",
            ".printdex.toml.tmp",
        ];
        for n in entries {
            fs::write(d.path().join(n), b"x").unwrap();
        }
        fs::create_dir(d.path().join(".dir.tmp-1")).unwrap();
        fs::write(d.path().join(".dir.tmp-1/inner"), b"x").unwrap();
        tree(d.path(), &[("sub/.c.tmp-1", b"x")]);
        let set_old = |rel: &str| {
            let f = fs::File::options()
                .write(true)
                .open(d.path().join(rel))
                .unwrap();
            f.set_modified(old).unwrap();
        };
        for n in [
            ".a.toml.tmp-0190aaaa",
            ".printdex-00ff.tmp",
            "keep.stl",
            ".hidden",
            "x.tmp-1",
        ] {
            set_old(n);
        }
        set_old(".printdex.toml.tmp");
        set_old("sub/.c.tmp-1");
        // A folder's mtime: set it last, after its contents were written.
        fs::File::open(d.path().join(".dir.tmp-1"))
            .and_then(|f| f.set_modified(old))
            .ok();

        sweep_temp_files(d.path(), Duration::from_secs(3600));

        let left = names(d.path());
        assert!(!left.contains(&".a.toml.tmp-0190aaaa".to_string()));
        assert!(!left.contains(&".printdex-00ff.tmp".to_string()));
        for keep in [
            ".b.json.tmp-0190bbbb",
            "keep.stl",
            ".hidden",
            "x.tmp-1",
            ".printdex.toml.tmp",
            "sub",
        ] {
            assert!(
                left.contains(&keep.to_string()),
                "{keep} was removed: {left:?}"
            );
        }
        assert!(
            d.path().join("sub/.c.tmp-1").exists(),
            "only one level deep"
        );
        #[cfg(unix)]
        assert!(!left.contains(&".dir.tmp-1".to_string()));
    }
}
