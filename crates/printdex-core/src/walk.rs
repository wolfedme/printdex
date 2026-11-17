//! Parallel library walk: lists files with size and mtime, reads no contents.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rayon::prelude::*;
use tokio_util::sync::CancellationToken;

use crate::error::{CoreError, Result};
use crate::fs::{EntryKind, Fs};

/// A printable mesh format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrintableFormat {
    Stl,
    ThreeMf,
    Obj,
}

impl PrintableFormat {
    /// Stable lower-case name, as stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            PrintableFormat::Stl => "stl",
            PrintableFormat::ThreeMf => "3mf",
            PrintableFormat::Obj => "obj",
        }
    }
}

/// What a file is, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Printable(PrintableFormat),
    /// A Printdex sidecar (`.printdex.toml` or `.<file>.printdex.toml`).
    Sidecar,
    Other,
}

/// Whether `file_name` is a Printdex sidecar.
pub fn is_sidecar_name(file_name: &str) -> bool {
    file_name == crate::sidecar::FOLDER_SIDECAR
        || (file_name.starts_with('.')
            && file_name.len() > crate::sidecar::FOLDER_SIDECAR.len() + 1
            && file_name.ends_with(crate::sidecar::FOLDER_SIDECAR))
}

/// One file found in a library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// Relative to the library root, `/`-separated, no leading `/`.
    pub rel_path: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub kind: FileKind,
}

/// Why a path was left out of a walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    NonUtf8,
    Unreadable,
}

/// A path the walk could not index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedPath {
    pub path: PathBuf,
    pub reason: SkipReason,
}

/// Result of a walk. `files` is sorted by `rel_path`.
#[derive(Debug, Default)]
pub struct WalkOutput {
    pub files: Vec<FileEntry>,
    pub skipped: Vec<SkippedPath>,
}

/// Folders that NAS systems, operating systems and archivers create and that
/// never hold a user's models. Matched ignoring ASCII case.
pub const IGNORED_DIR_NAMES: &[&str] = &[
    "@eaDir",
    "@Recycle",
    "@Recently-Snapshot",
    "#recycle",
    "#snapshot",
    "$RECYCLE.BIN",
    "System Volume Information",
    ".Trashes",
    "__MACOSX",
    "lost+found",
];

/// Which entries a walk looks at. Shared by full and subtree walks.
pub struct Rules;

impl Rules {
    /// Hidden folders and NAS/OS junk folders are skipped.
    pub fn keep_dir(name: &str) -> bool {
        !name.starts_with('.')
            && !IGNORED_DIR_NAMES
                .iter()
                .any(|j| j.eq_ignore_ascii_case(name))
    }

    /// Hidden files are skipped, except Printdex sidecars.
    pub fn keep_file(name: &str) -> bool {
        !name.starts_with('.') || is_sidecar_name(name)
    }
}

/// Classifies a file by its extension, ignoring ASCII case.
pub fn classify(file_name: &str) -> FileKind {
    if is_sidecar_name(file_name) {
        return FileKind::Sidecar;
    }
    let Some((stem, ext)) = file_name.rsplit_once('.') else {
        return FileKind::Other;
    };
    if stem.is_empty() {
        return FileKind::Other;
    }
    if ext.eq_ignore_ascii_case("stl") {
        FileKind::Printable(PrintableFormat::Stl)
    } else if ext.eq_ignore_ascii_case("3mf") {
        FileKind::Printable(PrintableFormat::ThreeMf)
    } else if ext.eq_ignore_ascii_case("obj") {
        FileKind::Printable(PrintableFormat::Obj)
    } else {
        FileKind::Other
    }
}

/// Whether the library root can be listed. An unmounted share or unplugged
/// drive must not look like an empty library, or every model would be
/// marked missing.
pub fn root_readable(fs: &dyn Fs, root: &Path) -> bool {
    fs.metadata(root).is_ok_and(|m| m.is_dir) && fs.read_dir(root).is_ok()
}

/// Walks `root` in parallel. Blocking; run it with `spawn_blocking`.
pub fn walk_library(fs: &dyn Fs, root: &Path, cancel: &CancellationToken) -> Result<WalkOutput> {
    walk_from(fs, root, &[String::new()], cancel)
}

/// Walks only the folders `subtrees` (relative to `root`, `""` for all of
/// it). A subtree that no longer exists yields no files.
pub fn walk_from(
    fs: &dyn Fs,
    root: &Path,
    subtrees: &[String],
    cancel: &CancellationToken,
) -> Result<WalkOutput> {
    if cancel.is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    if !root_readable(fs, root) {
        return Err(CoreError::LibraryOffline(root.to_path_buf()));
    }
    let output = Mutex::new(WalkOutput::default());
    subtrees.par_iter().for_each(|rel| {
        let dir = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(rel)
        };
        if !rel.is_empty() && fs.metadata(&dir).is_err() {
            // Gone (deleted or moved away): nothing to list.
            return;
        }
        visit(fs, &dir, rel, &output, cancel);
    });
    if cancel.is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    let mut output = output.into_inner().unwrap_or_else(|p| p.into_inner());
    output
        .files
        .sort_unstable_by(|a, b| a.rel_path.cmp(&b.rel_path));
    output.files.dedup_by(|a, b| a.rel_path == b.rel_path);
    output.skipped.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(output)
}

/// Lists `dir` (at `rel` inside the library), then its subfolders in
/// parallel. Symlinks are never followed, so loops cannot happen.
fn visit(fs: &dyn Fs, dir: &Path, rel: &str, out: &Mutex<WalkOutput>, cancel: &CancellationToken) {
    if cancel.is_cancelled() {
        return;
    }
    let entries = match fs.read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!(path = %dir.display(), error = %err, "skipping unreadable path");
            lock(out).skipped.push(SkippedPath {
                path: dir.to_path_buf(),
                reason: SkipReason::Unreadable,
            });
            return;
        }
    };
    let join = |name: &str| {
        if rel.is_empty() {
            name.to_owned()
        } else {
            format!("{rel}/{name}")
        }
    };
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for e in &entries {
        let Some(name) = e.name.to_str() else {
            lock(out).skipped.push(SkippedPath {
                path: dir.join(&e.name),
                reason: SkipReason::NonUtf8,
            });
            continue;
        };
        match e.kind {
            EntryKind::Dir if Rules::keep_dir(name) => dirs.push((dir.join(name), join(name))),
            EntryKind::File if Rules::keep_file(name) => {
                files.push((dir.join(name), join(name), classify(name)));
            }
            _ => {}
        }
    }
    files.par_iter().for_each(|(path, rel_path, kind)| {
        if cancel.is_cancelled() {
            return;
        }
        match fs.metadata(path) {
            Ok(meta) => lock(out).files.push(FileEntry {
                rel_path: rel_path.clone(),
                size: meta.len,
                mtime_ns: meta.mtime_ns,
                kind: *kind,
            }),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "cannot stat file");
                lock(out).skipped.push(SkippedPath {
                    path: path.clone(),
                    reason: SkipReason::Unreadable,
                });
            }
        }
    });
    dirs.par_iter()
        .for_each(|(path, rel)| visit(fs, path, rel, out, cancel));
}

fn lock(m: &Mutex<WalkOutput>) -> std::sync::MutexGuard<'_, WalkOutput> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::RealFs;
    use std::fs;

    fn touch(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    fn walk(root: &Path) -> WalkOutput {
        walk_library(&RealFs, root, &CancellationToken::new()).unwrap()
    }

    fn paths(out: &WalkOutput) -> Vec<&str> {
        out.files.iter().map(|f| f.rel_path.as_str()).collect()
    }

    #[test]
    fn classifies_uppercase_extensions() {
        assert_eq!(classify("A.STL"), FileKind::Printable(PrintableFormat::Stl));
        assert_eq!(
            classify("b.3MF"),
            FileKind::Printable(PrintableFormat::ThreeMf)
        );
        assert_eq!(classify("c.Obj"), FileKind::Printable(PrintableFormat::Obj));
        assert_eq!(classify("readme.md"), FileKind::Other);
        assert_eq!(classify("stl"), FileKind::Other);
    }

    #[test]
    fn walks_nested_files_with_forward_slashes() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "a/b/c.stl", b"12345");
        touch(d.path(), "a/x.png", b"12");
        let out = walk(d.path());
        assert_eq!(paths(&out), ["a/b/c.stl", "a/x.png"]);
        assert_eq!(out.files[0].size, 5);
        assert_eq!(out.files[0].kind, FileKind::Printable(PrintableFormat::Stl));
        assert_eq!(out.files[1].size, 2);
        assert_eq!(out.files[1].kind, FileKind::Other);
        assert!(out.files.iter().all(|f| f.mtime_ns > 0));
    }

    #[test]
    fn skips_hidden_entries_but_not_sidecars() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), ".hidden/a.stl", b"x");
        touch(d.path(), ".hidden/.printdex.toml", b"x");
        touch(d.path(), "m/.printdex.toml", b"x");
        touch(d.path(), "m/.hook.stl.printdex.toml", b"x");
        touch(d.path(), "m/.DS_Store", b"x");
        touch(d.path(), "m/.printdex.toml.tmp", b"x");
        let out = walk(d.path());
        assert_eq!(
            paths(&out),
            ["m/.hook.stl.printdex.toml", "m/.printdex.toml"]
        );
        assert!(out.files.iter().all(|f| f.kind == FileKind::Sidecar));
    }

    #[test]
    fn junk_folders_match_case_insensitively() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "$recycle.bin/x.stl", b"x");
        touch(d.path(), "@Recycle/x.stl", b"x");
        touch(d.path(), "@recently-snapshot/x.stl", b"x");
        touch(d.path(), "EADIR/keep.stl", b"x");
        assert_eq!(paths(&walk(d.path())), ["EADIR/keep.stl"]);
    }

    #[test]
    fn subtree_walk_lists_only_those_folders() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "a/x.stl", b"x");
        touch(d.path(), "b/y.stl", b"x");
        touch(d.path(), "b/c/z.stl", b"x");
        let out = walk_from(
            &RealFs,
            d.path(),
            &["b".into(), "gone".into()],
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(paths(&out), ["b/c/z.stl", "b/y.stl"]);
    }

    #[test]
    fn chaos_failures_skip_folders_not_the_walk() {
        let d = tempfile::tempdir().unwrap();
        for i in 0..20 {
            touch(d.path(), &format!("m{i}/a.stl"), b"x");
        }
        let fs = crate::fs::ChaosFs::new().failing_every(7);
        let out = walk_library(&fs, d.path(), &CancellationToken::new());
        // Either the root failed (offline) or some entries were skipped.
        if let Ok(out) = out {
            assert!(out.files.len() + out.skipped.len() >= 20);
        }
    }

    #[test]
    fn ignores_nas_junk_dirs() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "@eaDir/x.stl", b"x");
        touch(d.path(), "m/__MACOSX/y.stl", b"x");
        touch(d.path(), "#recycle/z.stl", b"x");
        assert!(walk(d.path()).files.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn does_not_follow_symlink_loops() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "m/a.stl", b"x");
        std::os::unix::fs::symlink("..", d.path().join("m/loop")).unwrap();
        assert_eq!(paths(&walk(d.path())), ["m/a.stl"]);
    }

    #[cfg(unix)]
    #[test]
    fn skips_unreadable_dirs() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "locked/x.stl", b"x");
        touch(d.path(), "ok/a.stl", b"x");
        let locked = d.path().join("locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read_dir(&locked).is_ok() {
            // Running as root: permissions are not enforced, nothing to test.
            fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let out = walk(d.path());
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(paths(&out), ["ok/a.stl"]);
        assert!(
            out.skipped
                .iter()
                .any(|s| s.path.ends_with("locked") && s.reason == SkipReason::Unreadable),
            "{:?}",
            out.skipped
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn skips_non_utf8_names() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join(OsStr::from_bytes(b"bad\xffname.stl")), b"x").unwrap();
        touch(d.path(), "good.stl", b"x");
        let out = walk(d.path());
        assert_eq!(paths(&out), ["good.stl"]);
        assert_eq!(out.skipped.len(), 1);
        assert_eq!(out.skipped[0].reason, SkipReason::NonUtf8);
    }

    #[test]
    fn missing_root_is_library_offline() {
        let d = tempfile::tempdir().unwrap();
        let r = walk_library(&RealFs, &d.path().join("gone"), &CancellationToken::new());
        assert!(matches!(r, Err(CoreError::LibraryOffline(_))));
    }

    #[test]
    fn cancelled_walk_returns_cancelled() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "a.stl", b"x");
        let c = CancellationToken::new();
        c.cancel();
        assert!(matches!(
            walk_library(&RealFs, d.path(), &c),
            Err(CoreError::Cancelled)
        ));
    }
}
