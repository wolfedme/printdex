//! Model snapshots per library.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use printdex_core::backup::secrets::Store;
use printdex_core::backup::{
    BackupConfig, BackupKind, BackupScope, Repo, RepoOptions, SnapshotFilter, Target, TargetConfig,
    TargetSecrets, default_models_on, models_tag,
};
use printdex_core::fs::RealFs;
use printdex_core::{Engine, LibraryId, LibraryKind};
use tokio_util::sync::CancellationToken;

struct Fx {
    dir: tempfile::TempDir,
    engine: Engine,
    lib: LibraryId,
    root: PathBuf,
    target: Target,
}

fn put(root: &Path, rel: &str, body: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, body).unwrap();
}

impl Fx {
    async fn new(files: &[(&str, &[u8])]) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        for (rel, body) in files {
            put(&root, rel, body);
        }
        let engine = Engine::open_with_cache(&dir.path().join("data"), &dir.path().join("cache"))
            .await
            .unwrap()
            .with_secret_store(Arc::new(Store::with_backend(None)));
        let lib = engine.add_library(&root, None).await.unwrap().id;
        engine.rescan(lib, CancellationToken::new()).await.unwrap();
        let target = Target::Folder {
            path: dir.path().join("backups"),
        };
        engine
            .configure_backup(BackupConfig {
                targets: vec![TargetConfig {
                    id: 0,
                    target: target.clone(),
                    encrypted: false,
                }],
                libraries: vec![(lib, true)],
                ..BackupConfig::default()
            })
            .await
            .unwrap();
        Fx {
            dir,
            engine,
            lib,
            root,
            target,
        }
    }

    /// Files in the library's newest model snapshot.
    fn backed_up(&self) -> BTreeSet<String> {
        let mut repo = Repo::open(
            &self.target,
            &TargetSecrets::default(),
            &RepoOptions::default(),
        )
        .unwrap();
        let snaps = repo
            .snapshots(&SnapshotFilter {
                host: None,
                tag: Some(models_tag(self.lib)),
            })
            .unwrap();
        let out = self.dir.path().join("out");
        repo.restore(
            &snaps.last().unwrap().id,
            "",
            &out,
            &CancellationToken::new(),
            None,
        )
        .unwrap();
        files(&out, &out)
    }
}

fn files(base: &Path, dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            out.extend(files(base, &p));
        } else {
            out.insert(
                p.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    out
}

#[tokio::test]
async fn backup_file_set_equals_scanner_file_set() {
    let fx = Fx::new(&[
        ("Hook/hook.stl", b"solid a\nendsolid a\n"),
        ("Hook/readme.txt", b"hello"),
        ("Hook/.printdex.toml", b"version = 1\n"),
        ("Single.stl", b"solid s\nendsolid s\n"),
        (".Single.stl.printdex.toml", b"version = 1\n"),
        ("Hook/.DS_Store", b"junk"),
        (".hidden/inner.stl", b"x"),
        ("@eaDir/thumb.jpg", b"x"),
        ("Hook/@EADIR/thumb.jpg", b"x"),
        ("#recycle/old.stl", b"x"),
        ("__MACOSX/Hook/._hook.stl", b"x"),
        ("Deep/Er/$RECYCLE.BIN/x.stl", b"x"),
        ("Deep/Er/part.3mf", b"x"),
    ])
    .await;
    let scanned: BTreeSet<String> =
        printdex_core::walk::walk_library(&RealFs, &fx.root, &CancellationToken::new())
            .unwrap()
            .files
            .into_iter()
            .map(|f| f.rel_path)
            .collect();
    let runs = fx
        .engine
        .backup_now(BackupScope::Models(fx.lib))
        .await
        .unwrap();
    let snap = runs[0].snapshot.as_ref().unwrap();
    assert_eq!(snap.tags, [models_tag(fx.lib)]);
    assert_eq!(fx.backed_up(), scanned);
    assert!(scanned.contains("Hook/.printdex.toml"));
    assert!(scanned.contains(".Single.stl.printdex.toml"));
    assert!(
        !scanned
            .iter()
            .any(|f| f.contains("DS_Store") || f.contains("eaDir"))
    );
}

#[tokio::test]
async fn offline_library_is_skipped() {
    let fx = Fx::new(&[("Hook/hook.stl", b"solid a\nendsolid a\n")]).await;
    std::fs::rename(&fx.root, fx.dir.path().join("unplugged")).unwrap();
    let runs = fx.engine.backup_now(BackupScope::All).await.unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].kind, BackupKind::Data);
    assert!(runs[0].snapshot.is_some(), "the data backup still runs");
    assert_eq!(runs[1].kind, BackupKind::Models(fx.lib));
    assert!(runs[1].snapshot.is_none());
    let history = fx.engine.backup_runs(5).await.unwrap();
    assert_eq!(history[0].result, "skipped");
    assert_eq!(history[1].result, "ok");
}

#[tokio::test]
async fn estimate_matches_library_size() {
    let fx = Fx::new(&[
        ("Hook/hook.stl", &[b'a'; 1000]),
        ("Hook/photo.jpg", &[b'b'; 300]),
        ("Bin.3mf", &[b'c'; 77]),
    ])
    .await;
    assert_eq!(fx.engine.estimate_models(fx.lib).await.unwrap(), 1377);
}

#[test]
fn defaults_on_for_local_off_for_network() {
    assert!(default_models_on(LibraryKind::Local));
    assert!(default_models_on(LibraryKind::Removable));
    assert!(!default_models_on(LibraryKind::Network));
}

#[tokio::test]
async fn models_off_means_no_model_snapshot() {
    let fx = Fx::new(&[("Hook/hook.stl", b"solid a\nendsolid a\n")]).await;
    let mut cfg = fx.engine.backup_config().await.unwrap();
    cfg.libraries = vec![(fx.lib, false)];
    fx.engine.configure_backup(cfg).await.unwrap();
    let runs = fx.engine.backup_now(BackupScope::All).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].kind, BackupKind::Data);
}
