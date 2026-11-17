//! Parallel library walk: lists files with size and mtime, reads no contents.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ignore::{WalkBuilder, WalkState};
use tokio_util::sync::CancellationToken;

use crate::error::{CoreError, Result};
use crate::time::system_time_to_ns;

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
/// never hold a user's models.
pub const IGNORED_DIR_NAMES: &[&str] = &[
    "@eaDir",
    "#recycle",
    "#snapshot",
    "$RECYCLE.BIN",
    "System Volume Information",
    ".Trashes",
    "__MACOSX",
    "lost+found",
];

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

/// Walks `root` in parallel. Blocking; run it with `spawn_blocking`.
pub fn walk_library(root: &Path, cancel: &CancellationToken) -> Result<WalkOutput> {
    if cancel.is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    // An unmounted share or unplugged drive must not look like an empty
    // library, or every model would be marked missing.
    let root_ok =
        std::fs::metadata(root).is_ok_and(|m| m.is_dir()) && std::fs::read_dir(root).is_ok();
    if !root_ok {
        return Err(CoreError::LibraryOffline(root.to_path_buf()));
    }

    let output = Mutex::new(WalkOutput::default());
    WalkBuilder::new(root)
        .standard_filters(false)
        // Hidden entries are skipped by hand below, so sidecars get through.
        .hidden(false)
        .follow_links(false)
        .filter_entry(|e| {
            let is_dir = e.file_type().is_some_and(|t| t.is_dir());
            let name = e.file_name().to_str();
            if e.depth() > 0 && name.is_some_and(|n| n.starts_with('.')) {
                return !is_dir && name.is_some_and(is_sidecar_name);
            }
            !(is_dir && name.is_some_and(|n| IGNORED_DIR_NAMES.contains(&n)))
        })
        .build_parallel()
        .run(|| {
            let output = &output;
            Box::new(move |entry| {
                if cancel.is_cancelled() {
                    return WalkState::Quit;
                }
                let entry = match entry {
                    Ok(e) => e,
                    Err(err) => {
                        let path = error_path(&err).unwrap_or_else(|| root.to_path_buf());
                        tracing::warn!(path = %path.display(), error = %err, "skipping unreadable path");
                        lock(output).skipped.push(SkippedPath {
                            path,
                            reason: SkipReason::Unreadable,
                        });
                        return WalkState::Continue;
                    }
                };
                if entry.depth() == 0 {
                    return WalkState::Continue;
                }
                let Some(file_type) = entry.file_type() else {
                    return WalkState::Continue;
                };
                let Some(rel_path) = rel_path(root, entry.path()) else {
                    lock(output).skipped.push(SkippedPath {
                        path: entry.path().to_path_buf(),
                        reason: SkipReason::NonUtf8,
                    });
                    // A directory with a non-UTF-8 name is skipped as a whole.
                    return WalkState::Skip;
                };
                if !file_type.is_file() {
                    return WalkState::Continue;
                }
                let meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(err) => {
                        tracing::warn!(path = %entry.path().display(), error = %err, "cannot stat file");
                        lock(output).skipped.push(SkippedPath {
                            path: entry.path().to_path_buf(),
                            reason: SkipReason::Unreadable,
                        });
                        return WalkState::Continue;
                    }
                };
                let kind = classify(entry.file_name().to_str().unwrap_or_default());
                let mtime_ns = meta.modified().map(system_time_to_ns).unwrap_or(0);
                lock(output).files.push(FileEntry {
                    rel_path,
                    size: meta.len(),
                    mtime_ns,
                    kind,
                });
                WalkState::Continue
            })
        });

    if cancel.is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    let mut output = output.into_inner().unwrap_or_else(|p| p.into_inner());
    output
        .files
        .sort_unstable_by(|a, b| a.rel_path.cmp(&b.rel_path));
    output.skipped.sort_unstable_by(|a, b| a.path.cmp(&b.path));
    Ok(output)
}

fn lock(m: &Mutex<WalkOutput>) -> std::sync::MutexGuard<'_, WalkOutput> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// `path` relative to `root` with `/` separators, or `None` when any
/// component is not valid UTF-8.
fn rel_path(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut out = String::new();
    for comp in rel.components() {
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(comp.as_os_str().to_str()?);
    }
    Some(out)
}

/// The path an `ignore` error refers to, if it names one.
fn error_path(err: &ignore::Error) -> Option<PathBuf> {
    match err {
        ignore::Error::WithPath { path, .. } => Some(path.clone()),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            error_path(err)
        }
        ignore::Error::Loop { child, .. } => Some(child.clone()),
        ignore::Error::Partial(errs) => errs.iter().find_map(error_path),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    fn walk(root: &Path) -> WalkOutput {
        walk_library(root, &CancellationToken::new()).unwrap()
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
        let r = walk_library(&d.path().join("gone"), &CancellationToken::new());
        assert!(matches!(r, Err(CoreError::LibraryOffline(_))));
    }

    #[test]
    fn cancelled_walk_returns_cancelled() {
        let d = tempfile::tempdir().unwrap();
        touch(d.path(), "a.stl", b"x");
        let c = CancellationToken::new();
        c.cancel();
        assert!(matches!(
            walk_library(d.path(), &c),
            Err(CoreError::Cancelled)
        ));
    }
}
