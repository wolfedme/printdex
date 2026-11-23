//! The one place for file writes and moves that must not lose data:
//! durable writes, a rename that never replaces, verified tree copies and
//! sweeping of leftover temporary files.
//!
//! Everything here is blocking (`std::fs`); async callers wrap it in
//! `spawn_blocking`. Every function calls [`faults::check`] before each real
//! operation, so tests can make the n-th one fail or change something just
//! before it runs (see [`faults`]).

pub mod faults;

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
/// With `UserData`, an `Err` from the final folder fsync means the rename
/// already happened: the new contents are in place, but may not survive a
/// power cut.
///
/// The `<name>` part of the temporary name is cut (on a character
/// boundary) so the whole name stays within 255 bytes.
///
/// The new file gets default permissions, not the old file's.
///
/// Fault injection: [`Op::Write`] and then [`Op::Rename`], both with `path`.
pub fn durable_write(path: &Path, bytes: &[u8], d: Durability) -> io::Result<()> {
    let tmp = temp_sibling(path, TEMP_INFIX)?;
    let dir = parent_dir(path);

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

/// Copies the file `from` to `to`, which must not exist (`AlreadyExists`,
/// and then nothing is touched) and whose folder must (`NotFound`; no folder
/// is created). The copy goes to a temporary sibling `.<name>.tmp-<uuid>`,
/// is fsynced and then moved to `to` with [`rename_no_replace`], so `to` is
/// either missing or complete. On any error the temporary is removed.
///
/// Fault injection: [`Op::Copy`] with `to`, then [`Op::Rename`] with `to`.
pub fn copy_file_durable(from: &Path, to: &Path) -> io::Result<()> {
    let tmp = temp_sibling(to, TEMP_INFIX)?;
    let mut w = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    let res = (|| {
        faults::check(Op::Copy, to)?;
        copy_file_hashed(from, &mut w)?;
        drop(w);
        rename_no_replace(&tmp, to)
    })();
    if let Err(e) = res {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    sync_dir(parent_dir(to))
}

/// A fresh temporary name next to `path`, of the kind [`durable_write`]
/// uses and [`sweep_temp_files`] removes.
pub(crate) fn temp_path(path: &Path) -> io::Result<PathBuf> {
    temp_sibling(path, TEMP_INFIX)
}

/// Infix of [`durable_write`]'s temporary files, which
/// [`sweep_temp_files`] removes.
const TEMP_INFIX: &str = ".tmp-";

/// Infix of the name a case-only rename parks the user's own entry under
/// between its two steps. [`sweep_temp_files`] never matches it: a leftover
/// is the user's file or folder, not a temporary.
const CASE_INFIX: &str = ".case-";

/// A fresh `.<name><infix><uuid v7 simple>` next to `path`, with `<name>`
/// cut on a character boundary so the whole name stays within 255 bytes. A
/// name that isn't valid Unicode is converted lossily; the name only has to
/// be unique and recognisable by its infix.
fn temp_sibling(path: &Path, infix: &str) -> io::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(
            ErrorKind::InvalidInput,
            format!("no file name in {}", path.display()),
        )
    })?;
    let name = name.to_string_lossy();
    let max = 255 - ".".len() - infix.len() - 32;
    let mut end = name.len().min(max);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    let uuid = uuid::Uuid::now_v7().simple();
    Ok(parent_dir(path).join(format!(".{}{infix}{uuid}", &name[..end])))
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
///   `EOPNOTSUPP` on Unix; `Unsupported`, `ERROR_INVALID_FUNCTION` or
///   `ERROR_NOT_SUPPORTED` from `hard_link` on FAT or SMB) and on other
///   platforms, it falls back to the check-rename-verify of
///   `rename_fallback`, which has a race window (see there).
///
/// A rename that only changes case ("Benchy" to "benchy" in the same
/// folder) works on case-insensitive filesystems too, where `to` "exists"
/// because it names `from` itself. When the no-replace rename reports
/// `AlreadyExists`, `from` and `to` are the same file (same device and
/// inode on Unix, same canonical path elsewhere) and their names differ
/// only in case, it renames in two no-replace steps through a temporary
/// sibling `.<name>.case-<uuid>`, moving it back to `from` if the second
/// step fails. A leftover `.<name>.case-*` (after a crash between the
/// steps, or if both the second step and the move back fail) is the user's
/// own entry and must never be swept; [`sweep_temp_files`] doesn't match
/// it. A hard link under a name differing only in case (possible
/// on case-sensitive filesystems) is therefore still refused: the second
/// step finds `to` in place.
///
/// Fault injection: [`Op::Rename`] with `to`, once per call (the two steps
/// of a case-only rename aren't counted separately).
pub fn rename_no_replace(from: &Path, to: &Path) -> io::Result<()> {
    faults::check(Op::Rename, to)?;
    match rename_no_replace_imp(from, to) {
        Err(e)
            if e.kind() == ErrorKind::AlreadyExists
                && is_case_only_alias(from, to, same_file(from, to)) =>
        {
            case_rename(from, to)
        }
        r => r,
    }
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
        // Not PermissionDenied: a locked or protected source must surface.
        Err(e)
            if e.kind() == ErrorKind::Unsupported || matches!(e.raw_os_error(), Some(1 | 50)) =>
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

/// Whether `to` is `from` under a name that differs only in case: the same
/// file (`same_file`), in the same folder, with names that differ but are
/// equal ignoring case. Both conditions are needed: equal-ignoring-case
/// names alone may be two distinct files, and the same file alone may be a
/// hard link under an unrelated name.
fn is_case_only_alias(from: &Path, to: &Path, same_file: bool) -> bool {
    if !same_file || from.parent() != to.parent() {
        return false;
    }
    match (
        from.file_name().and_then(|n| n.to_str()),
        to.file_name().and_then(|n| n.to_str()),
    ) {
        (Some(a), Some(b)) => a != b && a.to_lowercase() == b.to_lowercase(),
        _ => false,
    }
}

/// Whether `a` and `b` name the same file (not following a final symlink).
#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (fs::symlink_metadata(a), fs::symlink_metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Whether `a` and `b` name the same file: their canonical paths, which
/// carry the on-disk case, are equal.
#[cfg(not(unix))]
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// A case-only rename in two no-replace steps: `from` to a temporary
/// sibling, then to `to`. If the second step fails, the temporary is moved
/// back to `from`.
fn case_rename(from: &Path, to: &Path) -> io::Result<()> {
    let tmp = temp_sibling(from, CASE_INFIX)?;
    rename_no_replace_imp(from, &tmp)?;
    if let Err(e) = rename_no_replace_imp(&tmp, to) {
        if let Err(back) = rename_no_replace_imp(&tmp, from) {
            return Err(io::Error::new(
                back.kind(),
                format!(
                    "renaming {} to {} failed ({e}) and moving it back failed too ({back}); \
                     it is now at {}",
                    from.display(),
                    to.display(),
                    tmp.display()
                ),
            ));
        }
        return Err(e);
    }
    Ok(())
}

/// Check, rename, verify: for filesystems without an atomic no-replace
/// rename. Racy: something created at `to` between the check and the
/// rename is replaced, and the check afterwards can't tell, because `to`
/// exists either way. It only catches a rename that didn't move `from`.
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

/// The inner error of [`copy_tree_verified`]'s refusal of a tree holding
/// links or special files; see [`is_links_error`].
#[derive(Debug)]
struct LinksError;

impl std::fmt::Display for LinksError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("contains links or special files")
    }
}

impl std::error::Error for LinksError {}

/// Whether `e` is [`copy_tree_verified`]'s refusal of a tree holding links
/// or special files (not just any `Unsupported` error).
pub fn is_links_error(e: &io::Error) -> bool {
    e.kind() == ErrorKind::Unsupported
        && e.get_ref()
            .is_some_and(|inner| inner.downcast_ref::<LinksError>().is_some())
}

/// One entry of a tree to copy, relative to its root (empty for the root).
#[derive(Debug, PartialEq, Eq, Hash)]
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
/// (`InvalidData`); the source tree is then listed again, and a file or
/// folder added or removed during the copy fails it too (`InvalidData`).
/// On any failure what this call created at `to` is removed (never
/// something that appeared there meanwhile); `from` is never changed.
/// Permissions and timestamps are not copied.
///
/// Fault injection: [`Op::Copy`] before each file, with its destination
/// path. Files are copied in sorted, depth-first order.
pub fn copy_tree_verified(from: &Path, to: &Path) -> io::Result<()> {
    let entries = scan_tree(from)?;
    let root_is_dir = matches!(entries.first(), Some(Entry::Dir(_)));
    // Creating the root with `create_dir` / `create_new` also proves `to`
    // didn't exist, so the cleanup below only removes what this call made.
    if root_is_dir {
        fs::create_dir(to)?;
    } else if exists_no_follow(to)? {
        return Err(already_exists(to));
    }
    let mut created_root = root_is_dir;
    match copy_and_verify(from, to, &entries, &mut created_root) {
        Ok(()) => Ok(()),
        Err(e) => {
            if created_root {
                let _ = if root_is_dir {
                    fs::remove_dir_all(to)
                } else {
                    fs::remove_file(to)
                };
            }
            Err(e)
        }
    }
}

/// Lists the tree under `from` root first, sorted, refusing links and
/// special files.
fn scan_tree(from: &Path) -> io::Result<Vec<Entry>> {
    let refuse = || io::Error::new(ErrorKind::Unsupported, LinksError);
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

/// Copies `entries`, then verifies them. Sets `created_root` once a file
/// root has been created at `to`.
fn copy_and_verify(
    from: &Path,
    to: &Path,
    entries: &[Entry],
    created_root: &mut bool,
) -> io::Result<()> {
    let mut copied = Vec::new();
    for e in entries {
        match e {
            // The root was created by the caller.
            Entry::Dir(rel) if rel.as_os_str().is_empty() => {}
            Entry::Dir(rel) => fs::create_dir(to.join(rel))?,
            Entry::File(rel) => {
                let dst = under(to, rel);
                faults::check(Op::Copy, &dst)?;
                let mut w = OpenOptions::new().write(true).create_new(true).open(&dst)?;
                if rel.as_os_str().is_empty() {
                    *created_root = true;
                }
                let sum = copy_file_hashed(&under(from, rel), &mut w)?;
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
    if scan_tree(from)? != entries {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("{} changed during the copy", from.display()),
        ));
    }
    for e in entries {
        if let Entry::Dir(rel) = e {
            let _ = sync_dir(&under(to, rel));
        }
    }
    let _ = sync_dir(parent_dir(to));
    Ok(())
}

/// How a tree `to` relates to a tree `from`, as [`copy_state`] finds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyState {
    /// The same entries with the same contents.
    Complete,
    /// What an interrupted [`copy_tree_verified`] of `from` could have left:
    /// only entries `from` has, of the same kind, each file's bytes the
    /// start of the source file's.
    Partial,
    /// Anything else: something `from` doesn't have, or different bytes.
    NotACopy,
}

/// Compares `to` with `from` as a (possibly interrupted) copy of it, by
/// listing both trees and reading every file in `to`. Symlinks or special
/// files in `to` make it [`CopyState::NotACopy`]; in `from` they are an
/// error. Read-only.
pub fn copy_state(from: &Path, to: &Path) -> io::Result<CopyState> {
    let src = scan_tree(from)?;
    let dst = match scan_tree(to) {
        Ok(d) => d,
        Err(e) if is_links_error(&e) => return Ok(CopyState::NotACopy),
        Err(e) => return Err(e),
    };
    if std::mem::discriminant(&src[0]) != std::mem::discriminant(&dst[0]) {
        return Ok(CopyState::NotACopy);
    }
    let known: std::collections::HashSet<&Entry> = src.iter().collect();
    let mut complete = src.len() == dst.len();
    for e in &dst {
        if !known.contains(e) {
            return Ok(CopyState::NotACopy);
        }
        if let Entry::File(rel) = e {
            match prefix_of(&under(from, rel), &under(to, rel))? {
                None => return Ok(CopyState::NotACopy),
                Some(whole) => complete &= whole,
            }
        }
    }
    Ok(if complete {
        CopyState::Complete
    } else {
        CopyState::Partial
    })
}

/// Whether the file `part`'s bytes are the start of `whole`'s: `None` if
/// not, `Some(true)` if they are all of it.
fn prefix_of(whole: &Path, part: &Path) -> io::Result<Option<bool>> {
    let (mut w, mut p) = (File::open(whole)?, File::open(part)?);
    let (wl, pl) = (w.metadata()?.len(), p.metadata()?.len());
    if pl > wl {
        return Ok(None);
    }
    let (mut wb, mut pb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    let mut left = pl;
    while left > 0 {
        let n = (left as usize).min(pb.len());
        p.read_exact(&mut pb[..n])?;
        w.read_exact(&mut wb[..n])?;
        if pb[..n] != wb[..n] {
            return Ok(None);
        }
        left -= n as u64;
    }
    Ok(Some(pl == wl))
}

/// Size and xxh3-64 of a file's contents.
type Sum = (u64, u64);

/// Copies `src` into the new file `w`, fsyncs it, and returns the size and
/// hash of the bytes written.
fn copy_file_hashed(src: &Path, w: &mut File) -> io::Result<Sum> {
    let mut r = File::open(src)?;
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

/// Removes temporary files left behind by a crash: regular files in `dir`
/// (one level deep, not recursive) last modified more than `older_than`
/// ago, named exactly as this code names them: `.<name>.tmp-<32 lowercase
/// hex>` (a simple uuid, from [`durable_write`] and the sync store) or
/// `.printdex-<16 lowercase hex>.tmp` (the sidecar writer's older form).
/// Folders and symlinks are never removed, whatever their name, and
/// neither is `.<name>.case-<uuid>`, a user's entry parked by
/// [`rename_no_replace`]. Errors are logged and skipped; a missing or
/// unreadable `dir` does nothing.
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
        if !meta.is_file() || meta.modified().map_or(true, |m| m > cutoff) {
            continue;
        }
        let path = e.path();
        if faults::check(Op::Remove, &path).is_err() {
            continue;
        }
        if let Err(err) = fs::remove_file(&path) {
            tracing::debug!(path = %path.display(), %err, "could not sweep temporary file");
        }
    }
}

/// `.<name>.tmp-<32 lowercase hex>` (non-empty name) or
/// `.printdex-<16 lowercase hex>.tmp`.
fn is_temp_name(n: &str) -> bool {
    fn is_hex(s: &str, len: usize) -> bool {
        s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    }
    let Some(rest) = n.strip_prefix('.') else {
        return false;
    };
    if let Some(hex) = rest
        .strip_prefix("printdex-")
        .and_then(|r| r.strip_suffix(".tmp"))
        && is_hex(hex, 16)
    {
        return true;
    }
    // ".tmp-" plus 32 hex digits, after a non-empty name.
    let tail = ".tmp-".len() + 32;
    rest.len() > tail
        && rest.is_char_boundary(rest.len() - tail)
        && rest[rest.len() - tail..]
            .strip_prefix(".tmp-")
            .is_some_and(|hex| is_hex(hex, 32))
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
    fn copy_file_durable_copies_and_never_replaces() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("src.png"), d.path().join("dst.png"));
        fs::write(&from, b"pixels").unwrap();
        copy_file_durable(&from, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"pixels");
        assert_eq!(names(d.path()), ["dst.png", "src.png"]);
        fs::write(&from, b"other").unwrap();
        let err = copy_file_durable(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&to).unwrap(), b"pixels");
        assert_eq!(names(d.path()), ["dst.png", "src.png"]);
        // A failure leaves no temporary behind.
        let to2 = d.path().join("two.png");
        let g = faults::arm(&to2, Op::Copy, 0, Action::Fail(ErrorKind::StorageFull));
        let err = copy_file_durable(&from, &to2).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::StorageFull);
        assert_eq!(g.fired(), 1);
        assert_eq!(names(d.path()), ["dst.png", "src.png"]);
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
        assert!(is_links_error(&err));
        assert!(!is_links_error(&io::Error::new(
            ErrorKind::Unsupported,
            "contains links or special files"
        )));
        assert!(!to.exists());
    }

    #[test]
    fn copy_state_tells_copies_from_strangers() {
        let d = tempfile::tempdir().unwrap();
        let from = d.path().join("from");
        tree(&from, &[("a.stl", b"abcdef"), ("sub/b.stl", b"bb")]);
        let at = |name: &str, files: &[(&str, &[u8])]| {
            let p = d.path().join(name);
            fs::create_dir_all(&p).unwrap();
            tree(&p, files);
            p
        };
        let same = at("same", &[("a.stl", b"abcdef"), ("sub/b.stl", b"bb")]);
        let empty = at("empty", &[]);
        let part = at("part", &[("a.stl", b"abc")]);
        let other = at("other", &[("a.stl", b"abX")]);
        let extra = at("extra", &[("a.stl", b"abcdef"), ("new.stl", b"n")]);
        let longer = at("longer", &[("a.stl", b"abcdefg")]);
        assert_eq!(copy_state(&from, &same).unwrap(), CopyState::Complete);
        assert_eq!(copy_state(&from, &empty).unwrap(), CopyState::Partial);
        assert_eq!(copy_state(&from, &part).unwrap(), CopyState::Partial);
        assert_eq!(copy_state(&part, &from).unwrap(), CopyState::NotACopy);
        assert_eq!(copy_state(&from, &other).unwrap(), CopyState::NotACopy);
        assert_eq!(copy_state(&from, &extra).unwrap(), CopyState::NotACopy);
        assert_eq!(copy_state(&from, &longer).unwrap(), CopyState::NotACopy);
        // Single files.
        let (f1, f2, f3) = (
            d.path().join("f1"),
            d.path().join("f2"),
            d.path().join("f3"),
        );
        fs::write(&f1, b"hello").unwrap();
        fs::write(&f2, b"he").unwrap();
        fs::write(&f3, b"hello").unwrap();
        assert_eq!(copy_state(&f1, &f2).unwrap(), CopyState::Partial);
        assert_eq!(copy_state(&f1, &f3).unwrap(), CopyState::Complete);
        assert_eq!(copy_state(&f1, &from).unwrap(), CopyState::NotACopy);
    }

    #[test]
    fn instead_replaces_the_operation() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x");
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let g = faults::arm(
            d.path(),
            Op::Trash,
            0,
            Action::Instead(Box::new(move |p| {
                s2.lock().unwrap().push(p.to_path_buf());
                Ok(())
            })),
        );
        let mut ran = false;
        faults::perform(Op::Trash, &p, || {
            ran = true;
            Ok(())
        })
        .unwrap();
        assert!(!ran, "replaced");
        assert_eq!(seen.lock().unwrap().as_slice(), std::slice::from_ref(&p));
        assert_eq!(g.fired(), 1);
        faults::perform(Op::Trash, &p, || {
            ran = true;
            Ok(())
        })
        .unwrap();
        assert!(ran, "only once");
        // A Fail wins over an Instead on the same operation.
        let _i = faults::arm(
            d.path(),
            Op::Remove,
            0,
            Action::Instead(Box::new(|_| Ok(()))),
        );
        let _f = faults::arm(
            d.path(),
            Op::Remove,
            0,
            Action::Fail(ErrorKind::StorageFull),
        );
        let err = faults::perform(Op::Remove, &p, || Ok(())).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::StorageFull);
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
        const U: &str = "0190aaaabbbbccccddddeeeeffff0000"; // a uuid, simple
        let d = tempfile::tempdir().unwrap();
        let old = SystemTime::now() - Duration::from_secs(2 * 3600);
        let swept = [
            format!(".a.toml.tmp-{U}"),
            ".printdex-00000000000000ff.tmp".to_string(),
        ];
        let kept = [
            // Too new.
            format!(".b.json.tmp-{U}"),
            "keep.stl".to_string(),
            ".hidden".to_string(),
            "x.tmp-1".to_string(),
            ".printdex.toml.tmp".to_string(),
            // User files that only look like temporary files.
            ".notes.tmp-old".to_string(),
            format!(".c.tmp-{}", &U[1..]),
            format!(".c.tmp-{}", U.to_uppercase()),
            format!(".tmp-{U}"),
            ".printdex-00ff.tmp".to_string(),
            ".printdex-zzzzzzzzzzzzzzzz.tmp".to_string(),
        ];
        for n in swept.iter().chain(&kept) {
            fs::write(d.path().join(n), b"x").unwrap();
        }
        // A folder with a matching name is never removed.
        let dir_name = format!(".dir.tmp-{U}");
        tree(d.path(), &[(&format!("{dir_name}/inner"), b"x")]);
        tree(d.path(), &[(&format!("sub/.c.tmp-{U}"), b"x")]);
        let set_old = |rel: &str| {
            let f = fs::File::options()
                .write(true)
                .open(d.path().join(rel))
                .unwrap();
            f.set_modified(old).unwrap();
        };
        for n in swept.iter().chain(&kept[1..]) {
            set_old(n);
        }
        set_old(&format!("sub/.c.tmp-{U}"));
        // A folder's mtime: set it last, after its contents were written.
        fs::File::open(d.path().join(&dir_name))
            .and_then(|f| f.set_modified(old))
            .ok();

        sweep_temp_files(d.path(), Duration::from_secs(3600));

        let left = names(d.path());
        for n in &swept {
            assert!(!left.contains(n), "{n} was kept: {left:?}");
        }
        for n in kept.iter().chain([&dir_name, &"sub".to_string()]) {
            assert!(left.contains(n), "{n} was removed: {left:?}");
        }
        assert!(d.path().join(&dir_name).join("inner").exists());
        assert!(
            d.path().join(format!("sub/.c.tmp-{U}")).exists(),
            "only one level deep"
        );
    }

    #[test]
    fn durable_write_long_name_keeps_temp_name_short() {
        let d = tempfile::tempdir().unwrap();
        // 240 bytes of two-byte characters: the temporary name must be cut on
        // a character boundary and stay within 255 bytes.
        let name = "é".repeat(120);
        let p = d.path().join(&name);
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let s = seen.clone();
        let dir = d.path().to_path_buf();
        let _g = faults::arm(
            d.path(),
            Op::Write,
            0,
            Action::Before(Box::new(move |_| *s.lock().unwrap() = names(&dir))),
        );
        durable_write(&p, b"x", Durability::UserData).unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"x");
        assert_eq!(names(d.path()), [name]);
        let during = seen.lock().unwrap().clone();
        assert_eq!(during.len(), 1, "{during:?}");
        assert!(during[0].len() <= 255, "{}", during[0].len());
        assert!(is_temp_name(&during[0]), "{}", during[0]);
    }

    #[test]
    fn is_case_only_alias_needs_same_file_and_other_case() {
        let (a, b) = (Path::new("lib/Benchy"), Path::new("lib/benchy"));
        assert!(is_case_only_alias(a, b, true));
        // A different file that happens to differ only in case.
        assert!(!is_case_only_alias(a, b, false));
        // A hard link under a genuinely different name.
        assert!(!is_case_only_alias(a, Path::new("lib/other"), true));
        // The very same name.
        assert!(!is_case_only_alias(a, a, true));
        // Same name in a different folder.
        assert!(!is_case_only_alias(a, Path::new("other/benchy"), true));
    }

    #[test]
    fn case_rename_parking_name_is_never_swept() {
        let d = tempfile::tempdir().unwrap();
        for name in ["Benchy.stl".to_string(), "é".repeat(120)] {
            let p = d.path().join(&name);
            let parked = temp_sibling(&p, CASE_INFIX).unwrap();
            let parked = parked.file_name().unwrap().to_str().unwrap();
            assert!(parked.contains(".case-"), "{parked}");
            assert!(parked.len() <= 255, "{}", parked.len());
            assert!(!is_temp_name(parked), "{parked} would be swept");
            // The durable-write name for the same file still is.
            let tmp = temp_sibling(&p, TEMP_INFIX).unwrap();
            assert!(is_temp_name(tmp.file_name().unwrap().to_str().unwrap()));
        }
    }

    #[test]
    fn case_rename_two_step_moves_through_temp() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("Benchy"), d.path().join("benchy"));
        tree(&from, &[("a.stl", b"a")]);
        case_rename(&from, &to).unwrap();
        assert_eq!(names(d.path()), ["benchy"]);
        assert_eq!(fs::read(to.join("a.stl")).unwrap(), b"a");
    }

    #[cfg(unix)]
    #[test]
    fn rename_no_replace_refuses_hard_link_with_other_case() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("Benchy.stl"), d.path().join("benchy.stl"));
        fs::write(&from, b"solid").unwrap();
        if fs::hard_link(&from, &to).is_err() {
            return; // case-insensitive filesystem: the names are one entry
        }
        let err = rename_no_replace(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(names(d.path()), ["Benchy.stl", "benchy.stl"]);
        assert_eq!(fs::read(&from).unwrap(), b"solid");
    }

    #[test]
    fn rename_no_replace_refuses_distinct_file_with_other_case() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("Benchy.stl"), d.path().join("benchy.stl"));
        fs::write(&from, b"one").unwrap();
        fs::write(&to, b"two").unwrap();
        if names(d.path()).len() != 2 {
            return; // case-insensitive filesystem
        }
        let err = rename_no_replace(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&from).unwrap(), b"one");
        assert_eq!(fs::read(&to).unwrap(), b"two");
    }

    #[test]
    fn copy_tree_file_root_keeps_target_that_appeared() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("a.stl"), d.path().join("b.stl"));
        fs::write(&from, b"mine").unwrap();
        let t = to.clone();
        let _g = faults::arm(
            &to,
            Op::Copy,
            0,
            Action::Before(Box::new(move |_| fs::write(&t, b"theirs").unwrap())),
        );
        let err = copy_tree_verified(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&to).unwrap(), b"theirs");
        assert_eq!(fs::read(&from).unwrap(), b"mine");
    }

    #[test]
    fn copy_tree_detects_added_source_file() {
        let d = tempfile::tempdir().unwrap();
        let (from, to) = (d.path().join("from"), d.path().join("to"));
        tree(&from, &[("a.stl", b"a")]);
        let added = from.join("sub");
        let _g = faults::arm(
            &to,
            Op::Copy,
            0,
            Action::Before(Box::new(move |_| {
                tree(&added, &[("new.stl", b"n")]);
            })),
        );
        let err = copy_tree_verified(&from, &to).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidData);
        assert!(!to.exists());
        assert!(from.join("sub/new.stl").exists());
    }
}
