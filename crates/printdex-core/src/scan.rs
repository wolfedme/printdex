//! Scan pipeline: walk → diff against the database → fingerprint changed
//! parts on the CPU pool → group into models → write.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rayon::prelude::*;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::cpu::run_cpu;
use crate::db::{Db, KnownFile, Library};
use crate::error::{CoreError, Result};
use crate::events::{EngineEvent, ScanPhase};
use crate::fingerprint::{Fingerprint, fingerprint_file};
use crate::grouping::{ModelGroup, group_models};
use crate::time::now_ms;
use crate::walk::{FileEntry, FileKind, SkipReason, walk_library};

/// Files fingerprinted per CPU-pool job (one progress event each).
const FINGERPRINT_CHUNK: usize = 256;

/// Everything a scan writes to the database.
pub struct ScanWrite {
    pub files: Vec<FileEntry>,
    /// Index into `files`; only new or changed parts.
    pub fingerprints: HashMap<usize, Fingerprint>,
    pub groups: Vec<ModelGroup>,
    /// Relative paths (folders or files) the scan could not read this round.
    /// Existing rows at, under or containing them are not marked missing.
    pub keep: Vec<String>,
}

/// Model counts from applying a scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplyStats {
    pub models_added: u64,
    pub models_missing: u64,
    pub models_returned: u64,
}

/// Result of a finished scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanSummary {
    pub files_seen: u64,
    pub fingerprinted: u64,
    pub skipped: u64,
    pub models_added: u64,
    pub models_missing: u64,
    pub models_returned: u64,
    /// Sidecars read back into the database.
    pub sidecars_imported: u64,
    pub duration_ms: u64,
}

/// What a scan needs to read sidecars back: the writer (to rewrite merged
/// files) and this install's id.
#[derive(Clone, Copy, Debug)]
pub struct SidecarCtx<'a> {
    pub writer: &'a crate::sidecar::SidecarWriter,
    pub install: crate::ids::Uuid,
}

/// Whether `entry` must be read: a printable file that is new, changed in
/// size or mtime, or has no stored fingerprint.
pub fn needs_fingerprint(known: &HashMap<&str, &KnownFile>, entry: &FileEntry) -> bool {
    if !matches!(entry.kind, FileKind::Printable(_)) {
        return false;
    }
    match known.get(entry.rel_path.as_str()) {
        None => true,
        Some(k) => k.size != entry.size || k.mtime_ns != entry.mtime_ns || k.fp_hash.is_none(),
    }
}

/// Fingerprints `files[i]` for each `i` in `which` on the rayon pool.
/// Returns the successes and the indices that failed (vanished, truncated,
/// unreadable).
pub(crate) async fn fingerprint_entries(
    root: &Path,
    files: &[FileEntry],
    which: &[usize],
) -> (HashMap<usize, Fingerprint>, Vec<usize>) {
    let jobs: Vec<(usize, PathBuf)> = which
        .iter()
        .map(|&i| (i, root.join(&files[i].rel_path)))
        .collect();
    let results = run_cpu(move || {
        jobs.into_par_iter()
            .map(|(i, path)| {
                let r = fingerprint_file(&path);
                if let Err(e) = &r {
                    tracing::warn!(path = %path.display(), error = %e, "cannot fingerprint file, skipping");
                }
                (i, r.ok())
            })
            .collect::<Vec<_>>()
    })
    .await;
    let mut ok = HashMap::with_capacity(results.len());
    let mut failed = Vec::new();
    for (i, r) in results {
        match r {
            Some(fp) => {
                ok.insert(i, fp);
            }
            None => failed.push(i),
        }
    }
    failed.sort_unstable();
    (ok, failed)
}

/// Scans `lib` and writes the result. Emits `ScanStarted`, `ScanProgress`
/// and `ScanFinished`, or `ScanFailed` on error. Nothing is written when the
/// scan is cancelled or the library is offline.
pub async fn scan_library(
    db: &Db,
    lib: &Library,
    events: &broadcast::Sender<EngineEvent>,
    cancel: CancellationToken,
    sidecars: Option<SidecarCtx<'_>>,
) -> Result<ScanSummary> {
    let _ = events.send(EngineEvent::ScanStarted { library: lib.id });
    match run_scan_with(db, lib, events, cancel, sidecars, |_| {}).await {
        Ok(summary) => {
            let _ = events.send(EngineEvent::ScanFinished {
                library: lib.id,
                summary: summary.clone(),
            });
            Ok(summary)
        }
        Err(e) => {
            let _ = events.send(EngineEvent::ScanFailed {
                library: lib.id,
                error: std::sync::Arc::new(clone_error(&e)),
            });
            Err(e)
        }
    }
}

/// Copy of an error for event subscribers. `CoreError` is not `Clone`
/// because the I/O and database errors inside it are not; those are copied
/// by kind and message.
fn clone_error(e: &CoreError) -> CoreError {
    match e {
        CoreError::LibraryNotFound(id) => CoreError::LibraryNotFound(*id),
        CoreError::LibraryPathInvalid(p) => CoreError::LibraryPathInvalid(p.clone()),
        CoreError::LibraryOverlaps { existing } => CoreError::LibraryOverlaps {
            existing: existing.clone(),
        },
        CoreError::LibraryOffline(p) => CoreError::LibraryOffline(p.clone()),
        CoreError::AlreadyScanning(id) => CoreError::AlreadyScanning(*id),
        CoreError::Cancelled => CoreError::Cancelled,
        CoreError::LibraryNameInvalid => CoreError::LibraryNameInvalid,
        CoreError::TagInvalid => CoreError::TagInvalid,
        CoreError::CollectionNameInvalid => CoreError::CollectionNameInvalid,
        CoreError::CollectionExists => CoreError::CollectionExists,
        CoreError::CollectionNotFound => CoreError::CollectionNotFound,
        CoreError::ModelNotFound => CoreError::ModelNotFound,
        CoreError::AttemptNotFound => CoreError::AttemptNotFound,
        CoreError::SourceInvalid => CoreError::SourceInvalid,
        CoreError::TextTooLong => CoreError::TextTooLong,
        CoreError::SettingOutOfRange { field, min, max } => CoreError::SettingOutOfRange {
            field,
            min: *min,
            max: *max,
        },
        CoreError::PhotoInvalid => CoreError::PhotoInvalid,
        CoreError::Io { path, source } => CoreError::Io {
            path: path.clone(),
            source: std::io::Error::new(source.kind(), source.to_string()),
        },
        CoreError::Db(e) => CoreError::Db(sqlx::Error::Protocol(e.to_string())),
        CoreError::Migrate(e) => CoreError::Db(sqlx::Error::Protocol(e.to_string())),
    }
}

/// The scan, with `after_walk` called between the walk and fingerprinting so
/// tests can change the tree mid-scan.
async fn run_scan_with(
    db: &Db,
    lib: &Library,
    events: &broadcast::Sender<EngineEvent>,
    cancel: CancellationToken,
    sidecar_ctx: Option<SidecarCtx<'_>>,
    after_walk: impl FnOnce(&[FileEntry]),
) -> Result<ScanSummary> {
    let started = Instant::now();
    let progress = |phase: ScanPhase, done: usize, total: usize| {
        let _ = events.send(EngineEvent::ScanProgress {
            library: lib.id,
            phase,
            done: done as u64,
            total: total as u64,
        });
    };

    progress(ScanPhase::Walking, 0, 0);
    let root = lib.path.clone();
    let walk_cancel = cancel.clone();
    let walked = tokio::task::spawn_blocking(move || walk_library(&root, &walk_cancel))
        .await
        .map_err(|e| CoreError::Io {
            path: lib.path.clone(),
            source: std::io::Error::other(e.to_string()),
        })??;
    let mut files = walked.files;
    let files_seen = files.len();
    after_walk(&files);
    progress(ScanPhase::Walking, files_seen, files_seen);

    let known = db.known_files(lib.id).await?;
    // A library that had files and now yields none is far more likely an
    // unmounted share (empty mount point) or a share failing mid-walk than a
    // deliberately emptied folder. Refuse instead of marking it all missing.
    if files.is_empty() && !known.is_empty() {
        return Err(CoreError::LibraryOffline(lib.path.clone()));
    }
    let mut keep = Vec::new();
    for skipped in &walked.skipped {
        if skipped.reason != SkipReason::Unreadable {
            continue;
        }
        match skipped
            .path
            .strip_prefix(&lib.path)
            .ok()
            .and_then(rel_string)
        {
            Some(rel) if !rel.is_empty() => keep.push(rel),
            // The root itself (or something outside it) failed.
            _ => return Err(CoreError::LibraryOffline(lib.path.clone())),
        }
    }
    let known: HashMap<&str, &KnownFile> = known.iter().map(|k| (k.rel_path.as_str(), k)).collect();
    let which: Vec<usize> = (0..files.len())
        .filter(|&i| needs_fingerprint(&known, &files[i]))
        .collect();

    let mut fingerprints = HashMap::with_capacity(which.len());
    let mut failed = Vec::new();
    progress(ScanPhase::Fingerprinting, 0, which.len());
    for (n, chunk) in which.chunks(FINGERPRINT_CHUNK).enumerate() {
        if cancel.is_cancelled() {
            return Err(CoreError::Cancelled);
        }
        let (ok, bad) = fingerprint_entries(&lib.path, &files, chunk).await;
        fingerprints.extend(ok);
        failed.extend(bad);
        let done = (n * FINGERPRINT_CHUNK + chunk.len()).min(which.len());
        progress(ScanPhase::Fingerprinting, done, which.len());
    }
    let fingerprinted = fingerprints.len();

    // Drop files that vanished or changed mid-scan; re-key fingerprints to
    // the new indices.
    if !failed.is_empty() {
        keep.extend(failed.iter().map(|&i| files[i].rel_path.clone()));
        let failed: std::collections::HashSet<usize> = failed.iter().copied().collect();
        let mut remap = HashMap::with_capacity(fingerprints.len());
        let mut kept = Vec::with_capacity(files.len() - failed.len());
        for (i, f) in files.into_iter().enumerate() {
            if failed.contains(&i) {
                continue;
            }
            if let Some(fp) = fingerprints.remove(&i) {
                remap.insert(kept.len(), fp);
            }
            kept.push(f);
        }
        files = kept;
        fingerprints = remap;
    }

    if cancel.is_cancelled() {
        return Err(CoreError::Cancelled);
    }
    // The share may have dropped while we were reading; check again right
    // before writing so a vanished library is never marked missing.
    if !root_readable(&lib.path) {
        return Err(CoreError::LibraryOffline(lib.path.clone()));
    }
    let groups = group_models(&files);
    let sidecar_files: Vec<(String, i64)> = files
        .iter()
        .filter(|f| f.kind == FileKind::Sidecar)
        .map(|f| (f.rel_path.clone(), f.mtime_ns))
        .collect();
    progress(ScanPhase::Writing, 0, groups.len());
    let stats = db
        .apply_scan(
            lib.id,
            ScanWrite {
                files,
                fingerprints,
                groups,
                keep,
            },
        )
        .await?;
    db.set_last_full_scan(lib.id, now_ms()).await?;

    let mut sidecars_imported = 0;
    if let Some(ctx) = sidecar_ctx
        && lib.sidecar_enabled
    {
        let imported =
            crate::sidecar::import_library(db, &lib.path, lib.id, &sidecar_files, ctx.install)
                .await?;
        sidecars_imported = imported.imported;
        for id in imported.rewrite {
            ctx.writer.queue(id);
        }
        if imported.foreign {
            let _ = events.send(EngineEvent::SidecarForeign { library: lib.id });
        }
    }

    Ok(ScanSummary {
        files_seen: files_seen as u64,
        fingerprinted: fingerprinted as u64,
        skipped: (walked.skipped.len() + failed.len()) as u64,
        models_added: stats.models_added,
        models_missing: stats.models_missing,
        models_returned: stats.models_returned,
        sidecars_imported,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    })
}

/// `/`-joined UTF-8 relative path, or `None` if not representable.
fn rel_string(rel: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    parts.map(|p| p.join("/"))
}

fn root_readable(root: &Path) -> bool {
    std::fs::metadata(root).is_ok_and(|m| m.is_dir()) && std::fs::read_dir(root).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::LibraryKind;
    use crate::ids::LibraryId;
    use std::fs;

    struct Fixture {
        _dirs: tempfile::TempDir,
        root: PathBuf,
        db: Db,
        lib: Library,
        tx: broadcast::Sender<EngineEvent>,
    }

    impl Fixture {
        async fn new(files: &[&str]) -> Fixture {
            let dirs = tempfile::tempdir().unwrap();
            let root = dirs.path().join("lib");
            fs::create_dir_all(&root).unwrap();
            for f in files {
                write(&root, f, f.as_bytes());
            }
            let db = Db::open(&dirs.path().join("t.db")).await.unwrap();
            let lib = Library {
                id: LibraryId::new(),
                path: root.clone(),
                display_name: "lib".into(),
                kind: LibraryKind::Local,
                sidecar_enabled: true,
                last_full_scan_at: None,
            };
            db.insert_library(&lib).await.unwrap();
            let (tx, _) = broadcast::channel(256);
            Fixture {
                _dirs: dirs,
                root,
                db,
                lib,
                tx,
            }
        }

        async fn scan(&self) -> ScanSummary {
            scan_library(
                &self.db,
                &self.lib,
                &self.tx,
                CancellationToken::new(),
                None,
            )
            .await
            .unwrap()
        }

        async fn count(&self, sql: &'static str) -> i64 {
            sqlx::query_scalar(sql)
                .fetch_one(self.db.reader())
                .await
                .unwrap()
        }

        async fn model(&self, rel_path: &str) -> (Vec<u8>, Option<i64>) {
            sqlx::query_as("SELECT id, missing_since FROM models WHERE rel_path = ?")
                .bind(rel_path)
                .fetch_one(self.db.reader())
                .await
                .unwrap()
        }
    }

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    const TREE: &[&str] = &["Dragon/STLs/a.stl", "Dragon/cover.jpg", "Orc/orc.3mf"];

    #[tokio::test]
    async fn first_scan_indexes_models_parts_assets() {
        let f = Fixture::new(TREE).await;
        let s = f.scan().await;
        assert_eq!((s.files_seen, s.fingerprinted, s.models_added), (3, 2, 2));
        assert_eq!(f.count("SELECT count(*) FROM models").await, 2);
        assert_eq!(
            f.count("SELECT count(*) FROM parts WHERE fp_hash IS NOT NULL")
                .await,
            2
        );
        assert_eq!(f.count("SELECT count(*) FROM assets").await, 1);
        assert!(
            f.db.library(f.lib.id)
                .await
                .unwrap()
                .last_full_scan_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn unchanged_rescan_reads_no_contents() {
        let f = Fixture::new(TREE).await;
        f.scan().await;
        let s = f.scan().await;
        assert_eq!(
            (s.fingerprinted, s.models_added, s.models_missing),
            (0, 0, 0)
        );
    }

    #[tokio::test]
    async fn touched_file_is_refingerprinted() {
        let f = Fixture::new(TREE).await;
        f.scan().await;
        let q = "SELECT fp_hash FROM parts WHERE rel_path = 'Dragon/STLs/a.stl'";
        let before = f.count(q).await;
        write(
            &f.root,
            "Dragon/STLs/a.stl",
            b"different and longer content",
        );
        let s = f.scan().await;
        assert_eq!(s.fingerprinted, 1);
        assert_ne!(f.count(q).await, before);
    }

    #[tokio::test]
    async fn model_ids_are_stable_across_rescans() {
        let f = Fixture::new(TREE).await;
        f.scan().await;
        let before = f.model("Dragon").await.0;
        f.scan().await;
        assert_eq!(f.model("Dragon").await.0, before);
    }

    #[tokio::test]
    async fn deleted_folder_marks_model_missing_not_deleted() {
        let f = Fixture::new(TREE).await;
        f.scan().await;
        fs::remove_dir_all(f.root.join("Orc")).unwrap();
        let s = f.scan().await;
        assert_eq!(s.models_missing, 1);
        assert!(f.model("Orc").await.1.is_some());
        assert_eq!(
            f.count("SELECT count(*) FROM parts WHERE rel_path = 'Orc/orc.3mf' AND missing_since IS NOT NULL")
                .await,
            1
        );
        // A second scan does not count it again.
        assert_eq!(f.scan().await.models_missing, 0);
    }

    #[tokio::test]
    async fn restored_folder_returns() {
        let f = Fixture::new(TREE).await;
        f.scan().await;
        let id = f.model("Orc").await.0;
        fs::rename(
            f.root.join("Orc"),
            f.root.parent().unwrap().join("Orc-away"),
        )
        .unwrap();
        f.scan().await;
        fs::rename(
            f.root.parent().unwrap().join("Orc-away"),
            f.root.join("Orc"),
        )
        .unwrap();
        let s = f.scan().await;
        assert_eq!(s.models_returned, 1);
        assert_eq!(f.model("Orc").await, (id, None));
        assert_eq!(
            f.count("SELECT count(*) FROM parts WHERE missing_since IS NOT NULL")
                .await,
            0
        );
    }

    #[tokio::test]
    async fn scan_survives_file_deleted_before_fingerprint() {
        let f = Fixture::new(TREE).await;
        let walked = walk_library(&f.root, &CancellationToken::new()).unwrap();
        let gone = walked
            .files
            .iter()
            .position(|e| e.rel_path == "Orc/orc.3mf")
            .unwrap();
        fs::remove_file(f.root.join("Orc/orc.3mf")).unwrap();
        let all: Vec<usize> = (0..walked.files.len()).collect();
        let (ok, failed) = fingerprint_entries(&f.root, &walked.files, &all).await;
        assert_eq!(failed, [gone]);
        assert_eq!(ok.len(), 2);
        let s = f.scan().await;
        assert_eq!(s.models_added, 1);
    }

    #[tokio::test]
    async fn emits_started_progress_finished() {
        let f = Fixture::new(TREE).await;
        let mut rx = f.tx.subscribe();
        let summary = f.scan().await;
        let mut events = Vec::new();
        while let Ok(e) = rx.try_recv() {
            events.push(e);
        }
        assert!(matches!(
            events.first(),
            Some(EngineEvent::ScanStarted { .. })
        ));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, EngineEvent::ScanProgress { .. }))
        );
        match events.last() {
            Some(EngineEvent::ScanFinished { summary: s, .. }) => assert_eq!(*s, summary),
            other => panic!("unexpected last event {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_before_write_changes_nothing() {
        let f = Fixture::new(TREE).await;
        let c = CancellationToken::new();
        c.cancel();
        let r = scan_library(&f.db, &f.lib, &f.tx, c, None).await;
        assert!(matches!(r, Err(CoreError::Cancelled)));
        assert_eq!(f.count("SELECT count(*) FROM models").await, 0);
    }

    #[tokio::test]
    async fn empty_root_with_known_files_is_offline() {
        // An unmounted NAS share often leaves an empty mount-point folder.
        let f = Fixture::new(TREE).await;
        f.scan().await;
        for e in fs::read_dir(&f.root).unwrap() {
            fs::remove_dir_all(e.unwrap().path()).unwrap();
        }
        let r = scan_library(&f.db, &f.lib, &f.tx, CancellationToken::new(), None).await;
        assert!(matches!(r, Err(CoreError::LibraryOffline(_))), "{r:?}");
        assert_eq!(
            f.count("SELECT count(*) FROM models WHERE missing_since IS NOT NULL")
                .await,
            0
        );
    }

    /// Runs a scan with `after_walk` called between the walk and fingerprinting.
    async fn scan_with_hook(
        f: &Fixture,
        after_walk: impl FnOnce(&[FileEntry]),
    ) -> Result<ScanSummary> {
        run_scan_with(
            &f.db,
            &f.lib,
            &f.tx,
            CancellationToken::new(),
            None,
            after_walk,
        )
        .await
    }

    #[tokio::test]
    async fn file_vanishing_mid_scan_is_skipped_and_others_keep_their_fingerprints() {
        let f = Fixture::new(TREE).await;
        let s = scan_with_hook(&f, |_| fs::remove_file(f.root.join("Orc/orc.3mf")).unwrap())
            .await
            .unwrap();
        assert_eq!(
            (s.files_seen, s.fingerprinted, s.skipped, s.models_added),
            (3, 1, 1, 1)
        );
        let stored: i64 =
            sqlx::query_scalar("SELECT fp_hash FROM parts WHERE rel_path = 'Dragon/STLs/a.stl'")
                .fetch_one(f.db.reader())
                .await
                .unwrap();
        let real = fingerprint_file(&f.root.join("Dragon/STLs/a.stl"))
            .unwrap()
            .hash;
        assert_eq!(stored as u64, real);
        assert_eq!(
            f.count("SELECT count(*) FROM parts WHERE rel_path = 'Orc/orc.3mf'")
                .await,
            0
        );
    }

    #[tokio::test]
    async fn known_part_failing_fingerprint_is_not_marked_missing() {
        let f = Fixture::new(TREE).await;
        f.scan().await;
        write(
            &f.root,
            "Orc/orc.3mf",
            b"a newer, longer version of the orc",
        );
        let s = scan_with_hook(&f, |_| fs::remove_file(f.root.join("Orc/orc.3mf")).unwrap())
            .await
            .unwrap();
        assert_eq!((s.skipped, s.models_missing), (1, 0));
        assert_eq!(
            f.count("SELECT count(*) FROM parts WHERE missing_since IS NOT NULL")
                .await,
            0
        );
        assert_eq!(f.model("Orc").await.1, None);
    }

    #[tokio::test]
    async fn library_vanishing_mid_scan_is_offline() {
        let f = Fixture::new(TREE).await;
        f.scan().await;
        let away = f.root.parent().unwrap().join("away");
        let r = scan_with_hook(&f, |_| fs::rename(&f.root, &away).unwrap()).await;
        assert!(matches!(r, Err(CoreError::LibraryOffline(_))), "{r:?}");
        assert_eq!(
            f.count("SELECT count(*) FROM models WHERE missing_since IS NOT NULL")
                .await,
            0
        );
    }

    /// Whether this process can ignore file permissions (root on unix).
    #[cfg(unix)]
    fn permissions_enforced(dir: &Path) -> bool {
        use std::os::unix::fs::PermissionsExt;
        let probe = dir.join("probe");
        fs::create_dir_all(&probe).unwrap();
        fs::set_permissions(&probe, fs::Permissions::from_mode(0o000)).unwrap();
        let enforced = fs::read_dir(&probe).is_err();
        fs::set_permissions(&probe, fs::Permissions::from_mode(0o755)).unwrap();
        fs::remove_dir(&probe).unwrap();
        enforced
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unreadable_library_contents_are_offline() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new(TREE).await;
        if !permissions_enforced(f.root.parent().unwrap()) {
            return;
        }
        f.scan().await;
        // Root still listable, but nothing below it can be entered.
        for d in ["Dragon", "Orc"] {
            fs::set_permissions(f.root.join(d), fs::Permissions::from_mode(0o000)).unwrap();
        }
        let r = scan_library(&f.db, &f.lib, &f.tx, CancellationToken::new(), None).await;
        for d in ["Dragon", "Orc"] {
            fs::set_permissions(f.root.join(d), fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert!(matches!(r, Err(CoreError::LibraryOffline(_))), "{r:?}");
        assert_eq!(
            f.count("SELECT count(*) FROM models WHERE missing_since IS NOT NULL")
                .await,
            0
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unreadable_subfolder_keeps_its_models() {
        use std::os::unix::fs::PermissionsExt;
        let f = Fixture::new(TREE).await;
        if !permissions_enforced(f.root.parent().unwrap()) {
            return;
        }
        f.scan().await;
        fs::set_permissions(f.root.join("Orc"), fs::Permissions::from_mode(0o000)).unwrap();
        let r = scan_library(&f.db, &f.lib, &f.tx, CancellationToken::new(), None).await;
        fs::set_permissions(f.root.join("Orc"), fs::Permissions::from_mode(0o755)).unwrap();
        let s = r.unwrap();
        assert_eq!((s.skipped, s.models_missing), (1, 0));
        assert_eq!(
            f.count("SELECT count(*) FROM parts WHERE missing_since IS NOT NULL")
                .await,
            0
        );
    }

    #[test]
    fn needs_fingerprint_rules() {
        let e = FileEntry {
            rel_path: "a.stl".into(),
            size: 1,
            mtime_ns: 1,
            kind: FileKind::Printable(crate::walk::PrintableFormat::Stl),
        };
        let k = KnownFile {
            rel_path: "a.stl".into(),
            size: 1,
            mtime_ns: 1,
            fp_hash: Some(7),
            is_part: true,
        };
        let mut known = HashMap::new();
        assert!(needs_fingerprint(&known, &e));
        known.insert("a.stl", &k);
        assert!(!needs_fingerprint(&known, &e));
        let touched = FileEntry {
            mtime_ns: 2,
            ..e.clone()
        };
        assert!(needs_fingerprint(&known, &touched));
        let asset = FileEntry {
            kind: FileKind::Other,
            rel_path: "x.jpg".into(),
            ..e
        };
        assert!(!needs_fingerprint(&known, &asset));
    }
}
