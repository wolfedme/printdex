//! Restores: Printdex data (staged and swapped in on the next open),
//! libraries, and single models.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use printdex_core::backup::secrets::{MemoryBackend, Store};
use printdex_core::backup::{
    BackupConfig, BackupScope, FileChange, ModelRestoreDest, Target, TargetConfig, TargetSecrets,
};
use printdex_core::sidecar::SidecarFile;
use printdex_core::{
    AttemptResult, Engine, EngineEvent, LibraryId, Material, ModelId, NewAttempt, PrintSettings,
    Recipe,
};
use tokio_util::sync::CancellationToken;

const PASS: &str = "orbit violin zebra";

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

fn secrets() -> TargetSecrets {
    TargetSecrets {
        secret: None,
        passphrase: PASS.into(),
    }
}

struct Fx {
    dir: tempfile::TempDir,
    engine: Engine,
    lib: LibraryId,
    target: Target,
    store: Arc<Store>,
}

async fn open(data: &Path, store: &Arc<Store>) -> Engine {
    Engine::open(data)
        .await
        .unwrap()
        .with_secret_store(store.clone())
}

impl Fx {
    /// A library with a folder model (Bin), a single-file model (hook),
    /// backed up (data and models) to an encrypted folder target.
    async fn new() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        write(&root, "Bin/a.stl", "solid a\nendsolid a\n");
        write(&root, "Bin/b.stl", "solid b\nendsolid b\n");
        write(&root, "hook.stl", "solid hook\nendsolid hook\n");
        let store = Arc::new(Store::with_backend(Some(
            Arc::new(MemoryBackend::default()),
        )));
        let engine = open(&dir.path().join("data"), &store).await;
        let lib = engine.add_library(&root, None).await.unwrap().id;
        engine.rescan(lib, CancellationToken::new()).await.unwrap();
        let target = Target::Folder {
            path: dir.path().join("backups"),
        };
        let cfg = engine
            .configure_backup(BackupConfig {
                targets: vec![TargetConfig {
                    id: 0,
                    target: target.clone(),
                    encrypted: true,
                }],
                libraries: vec![(lib, true)],
                ..BackupConfig::default()
            })
            .await
            .unwrap();
        engine.set_target_secrets(cfg.targets[0].id, &secrets(), true);
        Fx {
            dir,
            engine,
            lib,
            target,
            store,
        }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("lib")
    }

    async fn model(engine: &Engine, name: &str) -> ModelId {
        engine
            .search(name, 10)
            .await
            .unwrap()
            .into_iter()
            .find(|h| h.name == name)
            .unwrap_or_else(|| panic!("no model {name}"))
            .id
    }

    async fn backup(&self) {
        self.engine.flush_sidecars().await;
        self.engine.backup_now(BackupScope::All).await.unwrap();
    }

    /// Restores the newest data snapshot into a fresh data folder, then
    /// opens it (which swaps the restore in).
    async fn restore_fresh(&self) -> (PathBuf, Engine) {
        let data = self.dir.path().join("data2");
        let fresh = open(&data, &self.store).await;
        let snaps = fresh
            .data_snapshots(&self.target, &secrets())
            .await
            .unwrap();
        assert!(!snaps.is_empty());
        fresh
            .restore_data(&self.target, &secrets(), &snaps[0].id, true)
            .await
            .unwrap();
        drop(fresh);
        let engine = open(&data, &self.store).await;
        (data, engine)
    }
}

fn png(path: &Path) {
    image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3]))
        .save(path)
        .unwrap();
}

fn attempt() -> NewAttempt {
    NewAttempt {
        printed_at: 1_763_000_000_000,
        result: AttemptResult::Success,
        settings: PrintSettings::default(),
        duration_min: Some(30),
        notes: "Clean".into(),
        parts: vec![],
    }
}

#[tokio::test]
async fn full_restore_round_trip() {
    let fx = Fx::new().await;
    let bin = Fx::model(&fx.engine, "Bin").await;
    fx.engine.add_tag(bin, "gridfinity").await.unwrap();
    let recipe = Recipe {
        settings: PrintSettings {
            material: Some(Material::Petg),
            ..PrintSettings::default()
        },
        ..Recipe::default()
    };
    fx.engine.set_recipe(bin, Some(&recipe)).await.unwrap();
    let a = fx.engine.log_attempt(bin, &attempt()).await.unwrap();
    let photo = fx.dir.path().join("p.png");
    png(&photo);
    fx.engine.add_photo(a, &photo).await.unwrap();
    let settings = fx.dir.path().join("conf/settings.toml");
    write(
        settings.parent().unwrap(),
        "settings.toml",
        "mode = \"dark\"\n",
    );
    fx.engine.set_backup_files(vec![settings]);
    let before = fx.engine.model(bin).await.unwrap().unwrap();
    fx.backup().await;

    let data = fx.dir.path().join("data2");
    let fresh = open(&data, &fx.store).await;
    let snaps = fresh.data_snapshots(&fx.target, &secrets()).await.unwrap();
    let restored = fresh
        .restore_data(&fx.target, &secrets(), &snaps[0].id, true)
        .await
        .unwrap();
    assert_eq!(restored.settings_files.len(), 1);
    assert!(restored.settings_files[0].ends_with("settings.toml"));
    drop(fresh);
    let engine = open(&data, &fx.store).await;

    let bin2 = Fx::model(&engine, "Bin").await;
    assert_eq!(bin2, bin);
    let after = engine.model(bin2).await.unwrap().unwrap();
    assert_eq!(after.number, before.number);
    let md = engine.model_data(bin2).await.unwrap().unwrap();
    assert_eq!(md.tags, ["gridfinity"]);
    assert_eq!(md.recipe.unwrap().settings, recipe.settings);
    assert_eq!(md.attempts.len(), 1);
    assert_eq!(md.attempts[0].photos.len(), 1);
    assert!(
        fs::read_dir(data.join("photos")).unwrap().next().is_some(),
        "photos restored"
    );
    // Each library asks where it lives now.
    let pending = engine.pending_relinks().await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, fx.lib);
    // The staging folder is gone and the old data was kept aside.
    assert!(!data.join("restore-staging").exists());
    assert!(
        fs::read_dir(&data)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("pre-restore-"))
    );
}

#[tokio::test]
async fn restored_install_gets_new_id_with_predecessor() {
    let fx = Fx::new().await;
    fx.backup().await;
    let (data, engine) = fx.restore_fresh().await;
    assert_ne!(engine.install_id(), fx.engine.install_id());
    let preds = engine.install_predecessors().await.unwrap();
    assert!(
        preds.contains(&fx.engine.install_id().to_string()),
        "{preds:?}"
    );
    // It keeps backing up to the same place, with the remembered secrets.
    let cfg = engine.backup_config().await.unwrap();
    assert_eq!(cfg.targets.len(), 1);
    assert_eq!(cfg.targets[0].target, fx.target);
    assert!(engine.target_secrets_known(&cfg.targets[0]));
    engine.backup_now(BackupScope::Data).await.unwrap();
    // A successful backup clears the data the restore replaced.
    assert!(
        !fs::read_dir(&data)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with("pre-restore-"))
    );
}

#[tokio::test]
async fn predecessor_sidecars_are_own() {
    let fx = Fx::new().await;
    let bin = Fx::model(&fx.engine, "Bin").await;
    fx.engine.add_tag(bin, "mine").await.unwrap();
    fx.backup().await;
    let (_data, engine) = fx.restore_fresh().await;
    engine.relink_library(fx.lib, &fx.root()).await.unwrap();
    engine.flush_sidecars().await;
    // A sidecar the old install wrote, edited later.
    let path = fx.root().join("Bin/.printdex.toml");
    let mut file = SidecarFile::parse(&fs::read_to_string(&path).unwrap()).unwrap();
    file.writer = fx.engine.install_id().to_string();
    file.tags = vec!["mine".into(), "edited".into()];
    file.updated_at += 60_000;
    fs::write(&path, file.to_toml().unwrap()).unwrap();
    let mut rx = engine.subscribe();
    engine
        .rescan(fx.lib, CancellationToken::new())
        .await
        .unwrap();
    while let Ok(e) = rx.try_recv() {
        assert!(
            !matches!(e, EngineEvent::SidecarForeign { .. }),
            "predecessor's sidecar counted as foreign"
        );
    }
    let tags = engine.model_data(bin).await.unwrap().unwrap().tags;
    assert_eq!(tags, ["edited", "mine"]);
}

#[tokio::test]
async fn restore_beats_newer_sidecars() {
    let fx = Fx::new().await;
    let bin = Fx::model(&fx.engine, "Bin").await;
    fx.engine.add_tag(bin, "old").await.unwrap();
    fx.backup().await;
    // Changed after the backup: the sidecar on disk is newer.
    fx.engine.add_tag(bin, "newer").await.unwrap();
    fx.engine.flush_sidecars().await;
    let path = fx.root().join("Bin/.printdex.toml");
    assert!(fs::read_to_string(&path).unwrap().contains("newer"));

    let (_data, engine) = fx.restore_fresh().await;
    engine.relink_library(fx.lib, &fx.root()).await.unwrap();
    engine.flush_sidecars().await;
    assert_eq!(engine.model_data(bin).await.unwrap().unwrap().tags, ["old"]);
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains("newer"), "{text}");
    // Later scans import edits as usual.
    engine
        .rescan(fx.lib, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(engine.model_data(bin).await.unwrap().unwrap().tags, ["old"]);
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in fs::read_dir(from).unwrap() {
        let p = entry.unwrap().path();
        let dest = to.join(p.file_name().unwrap());
        if p.is_dir() {
            fs::create_dir_all(&dest).unwrap();
            copy_tree(&p, &dest);
        } else {
            fs::create_dir_all(to).unwrap();
            fs::copy(&p, &dest).unwrap();
        }
    }
}

#[tokio::test]
async fn relink_rescans_and_keeps_identity() {
    let fx = Fx::new().await;
    let bin = Fx::model(&fx.engine, "Bin").await;
    let hook = Fx::model(&fx.engine, "hook").await;
    fx.engine.add_tag(bin, "kept").await.unwrap();
    fx.backup().await;
    let (_data, engine) = fx.restore_fresh().await;
    // The library now lives somewhere else.
    let moved = fx.dir.path().join("nas/lib");
    fs::create_dir_all(&moved).unwrap();
    copy_tree(&fx.root(), &moved);
    let summary = engine.relink_library(fx.lib, &moved).await.unwrap();
    assert_eq!(summary.models_added, 0);
    assert_eq!(summary.models_missing, 0);
    for id in [bin, hook] {
        let m = engine.model(id).await.unwrap().unwrap();
        assert_eq!(m.missing_since, None);
    }
    assert_eq!(
        engine.model_data(bin).await.unwrap().unwrap().tags,
        ["kept"]
    );
    let lib = engine
        .libraries()
        .await
        .unwrap()
        .into_iter()
        .find(|l| l.id == fx.lib)
        .unwrap();
    assert_eq!(lib.path, dunce::canonicalize(&moved).unwrap());
    assert!(engine.pending_relinks().await.unwrap().is_empty());
}

#[tokio::test]
async fn restore_library_files_into_new_folder() {
    let fx = Fx::new().await;
    fx.backup().await;
    let (_data, engine) = fx.restore_fresh().await;
    let snaps = engine.library_snapshots(fx.lib).await.unwrap();
    assert_eq!(snaps.len(), 1);
    let dest = fx.dir.path().join("restored-lib");
    let summary = engine
        .restore_library_files(fx.lib, &snaps[0].id, &dest)
        .await
        .unwrap();
    assert_eq!(summary.models_missing, 0);
    assert_eq!(
        fs::read_to_string(dest.join("Bin/a.stl")).unwrap(),
        "solid a\nendsolid a\n"
    );
}

#[tokio::test]
async fn model_restore_trashes_changed_files_only() {
    use printdex_core::fsx::faults::{self, Action, Op};
    let fx = Fx::new().await;
    let bin = Fx::model(&fx.engine, "Bin").await;
    fx.backup().await;
    let snaps = fx.engine.model_snapshots(bin).await.unwrap();
    assert_eq!(snaps.len(), 1);
    let broken = "solid broken\nendsolid broken\n";
    write(&fx.root(), "Bin/a.stl", broken);
    // Keep the system trash out of it: the changed file goes to a folder.
    let trash = fx.dir.path().join(".test-trash");
    let trashed = {
        let trash = trash.clone();
        faults::arm(
            &fx.root(),
            Op::Trash,
            0,
            Action::Instead(Box::new(move |p| {
                fs::create_dir_all(&trash)?;
                fs::rename(p, trash.join("0"))
            })),
        )
    };
    let b_mtime = fs::metadata(fx.root().join("Bin/b.stl"))
        .unwrap()
        .modified()
        .unwrap();

    let preview = fx
        .engine
        .model_restore_preview(bin, &snaps[0].id)
        .await
        .unwrap();
    let change = |rel: &str| preview.iter().find(|d| d.rel == rel).map(|d| d.change);
    assert_eq!(change("a.stl"), Some(FileChange::Changed));
    assert_eq!(change("b.stl"), Some(FileChange::Same));

    let written = fx
        .engine
        .restore_model(bin, &snaps[0].id, ModelRestoreDest::Original)
        .await
        .unwrap();
    let rels: Vec<_> = written.iter().map(|d| d.rel.as_str()).collect();
    assert_eq!(rels, ["a.stl"]);
    assert_eq!(
        trashed.fired(),
        1,
        "only the changed file went to the trash"
    );
    assert_eq!(fs::read_to_string(trash.join("0")).unwrap(), broken);
    assert_eq!(
        fs::read_to_string(fx.root().join("Bin/a.stl")).unwrap(),
        "solid a\nendsolid a\n"
    );
    // The identical file was left alone.
    assert_eq!(
        fs::metadata(fx.root().join("Bin/b.stl"))
            .unwrap()
            .modified()
            .unwrap(),
        b_mtime
    );

    // Another folder gets a copy and leaves the library alone.
    let other = fx.dir.path().join("elsewhere");
    fx.engine
        .restore_model(bin, &snaps[0].id, ModelRestoreDest::Folder(other.clone()))
        .await
        .unwrap();
    assert!(other.join("Bin/a.stl").is_file());
    assert!(other.join("Bin/b.stl").is_file());
}

/// Restoring a model's changed file fails while the restored copy is
/// written (another drive, disk full): the user's file is still there,
/// untouched and not in the trash, and no temporary file is left.
#[tokio::test]
async fn failed_model_restore_keeps_the_original() {
    use printdex_core::fsx::faults::{self, Action, Op};
    use std::io::ErrorKind;
    let fx = Fx::new().await;
    let bin = Fx::model(&fx.engine, "Bin").await;
    fx.backup().await;
    let snaps = fx.engine.model_snapshots(bin).await.unwrap();
    let broken = "solid broken\nendsolid broken\n";
    write(&fx.root(), "Bin/a.stl", broken);
    let root = fx.root();
    let trash = fx.dir.path().join(".test-trash");
    let _cross = faults::arm(
        &root,
        Op::Rename,
        0,
        Action::Fail(ErrorKind::CrossesDevices),
    );
    let full = faults::arm(&root, Op::Copy, 0, Action::Fail(ErrorKind::StorageFull));
    let _trash = {
        let trash = trash.clone();
        faults::arm(
            &root,
            Op::Trash,
            0,
            Action::Instead(Box::new(move |p| {
                fs::create_dir_all(&trash)?;
                fs::rename(p, trash.join("0"))
            })),
        )
    };
    let r = fx
        .engine
        .restore_model(bin, &snaps[0].id, ModelRestoreDest::Original)
        .await;
    assert!(r.is_err(), "{r:?}");
    assert_eq!(full.fired(), 1, "the restored copy went through fsx");
    assert_eq!(
        fs::read_to_string(fx.root().join("Bin/a.stl")).unwrap(),
        broken
    );
    assert!(!trash.exists(), "nothing went to the trash");
    let names: Vec<_> = fs::read_dir(fx.root().join("Bin"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.contains("tmp")), "{names:?}");
}

/// The changed file went to the trash, then the restored copy can't take
/// its name: the copy is kept under a name the temp sweep leaves alone,
/// and the error says where.
#[tokio::test]
async fn restored_copy_is_kept_when_the_last_rename_fails() {
    use printdex_core::fsx::faults::{self, Action, Op};
    use std::io::ErrorKind;
    let fx = Fx::new().await;
    let bin = Fx::model(&fx.engine, "Bin").await;
    fx.backup().await;
    let snaps = fx.engine.model_snapshots(bin).await.unwrap();
    write(&fx.root(), "Bin/a.stl", "solid broken\nendsolid broken\n");
    let root = fx.root();
    let trash = fx.dir.path().join(".test-trash");
    let _trash = {
        let trash = trash.clone();
        faults::arm(
            &root,
            Op::Trash,
            0,
            Action::Instead(Box::new(move |p| {
                fs::create_dir_all(&trash)?;
                fs::rename(p, trash.join("0"))
            })),
        )
    };
    // Rename 0 puts the restored copy beside a.stl; rename 1 would give it
    // a.stl's name.
    let last = faults::arm(
        &root,
        Op::Rename,
        1,
        Action::Fail(ErrorKind::PermissionDenied),
    );
    let r = fx
        .engine
        .restore_model(bin, &snaps[0].id, ModelRestoreDest::Original)
        .await;
    let err = format!("{}", r.unwrap_err());
    assert_eq!(last.fired(), 1);
    assert!(
        trash.join("0").is_file(),
        "the changed file is in the trash"
    );
    let bin_dir = fx.root().join("Bin");
    let kept: Vec<_> = fs::read_dir(&bin_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".a.stl.restored-")
        })
        .collect();
    assert_eq!(
        kept.len(),
        1,
        "{:?}",
        fs::read_dir(&bin_dir).unwrap().collect::<Vec<_>>()
    );
    assert_eq!(
        fs::read_to_string(&kept[0]).unwrap(),
        "solid a\nendsolid a\n"
    );
    assert!(err.contains(".a.stl.restored-"), "{err}");
    printdex_core::fsx::sweep_temp_files(&bin_dir, std::time::Duration::ZERO);
    assert!(kept[0].is_file(), "the sweep leaves it alone");
}

#[tokio::test]
async fn deleted_model_restores_from_backup() {
    let fx = Fx::new().await;
    let hook = Fx::model(&fx.engine, "hook").await;
    fx.engine.add_tag(hook, "hanger").await.unwrap();
    fx.backup().await;
    fs::remove_file(fx.root().join("hook.stl")).unwrap();
    fx.engine
        .rescan(fx.lib, CancellationToken::new())
        .await
        .unwrap();
    assert!(
        fx.engine
            .model(hook)
            .await
            .unwrap()
            .unwrap()
            .missing_since
            .is_some()
    );

    let snaps = fx.engine.model_snapshots(hook).await.unwrap();
    assert_eq!(snaps.len(), 1);
    fx.engine
        .restore_model(hook, &snaps[0].id, ModelRestoreDest::Original)
        .await
        .unwrap();
    assert!(fx.root().join("hook.stl").is_file());
    let m = fx.engine.model(hook).await.unwrap().unwrap();
    assert_eq!(m.missing_since, None);
    assert_eq!(
        fx.engine.model_data(hook).await.unwrap().unwrap().tags,
        ["hanger"]
    );
}

mod swap {
    use super::*;
    use printdex_core::fsx::faults::{self, Action, Op};
    use std::io::ErrorKind;

    /// Creates a database in `dir`, closes it (so nothing is left in its
    /// `-wal`) and returns its install id.
    async fn db_at(dir: &Path) -> String {
        fs::create_dir_all(dir).unwrap();
        let db = printdex_core::db::Db::open(&dir.join("printdex.db"))
            .await
            .unwrap();
        let id = db.install_id().await.unwrap().to_string();
        db.close().await;
        id
    }

    /// A data folder holding an old install (its database, `photos/old.png`
    /// and a stale `-wal`) and a staged restore of another one (`photos/new.png`).
    /// Returns the folder and the old and restored install ids.
    async fn staged() -> (tempfile::TempDir, PathBuf, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let other = dir.path().join("other");
        let old_id = db_at(&data).await;
        write(&data, "photos/old.png", "old photo");
        write(&data, "printdex.db-wal", "stale wal");
        let new_id = db_at(&other).await;
        let staging = data.join("restore-staging");
        fs::create_dir_all(&staging).unwrap();
        fs::rename(other.join("printdex.db"), staging.join("printdex.db")).unwrap();
        write(&staging, "photos/new.png", "new photo");
        fs::write(staging.join(".ready"), b"").unwrap();
        (dir, data, old_id, new_id)
    }

    fn asides(data: &Path) -> Vec<PathBuf> {
        fs::read_dir(data)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("pre-restore-")
            })
            .collect()
    }

    async fn check_done(data: &Path, new_id: &str) {
        let engine = Engine::open(data).await.unwrap();
        assert_eq!(engine.install_id().to_string(), new_id, "restored db");
        drop(engine);
        assert!(data.join("photos/new.png").is_file());
        assert!(!data.join("photos/old.png").exists());
        assert!(!data.join("restore-staging").exists());
        let aside = asides(data);
        assert_eq!(aside.len(), 1, "one pre-restore folder");
        assert!(aside[0].join("printdex.db").is_file());
        assert!(aside[0].join("photos/old.png").is_file());
        assert_eq!(
            fs::read_to_string(aside[0].join("printdex.db-wal")).unwrap(),
            "stale wal"
        );
    }

    #[tokio::test]
    async fn interrupted_swap_keeps_restored_photos() {
        for k in 0..50 {
            let (_dir, data, _old, new_id) = staged().await;
            let guard = faults::arm(&data, Op::Rename, k, Action::Fail(ErrorKind::Other));
            let first = Engine::open(&data).await;
            let fired = guard.fired();
            drop(guard);
            if fired == 0 {
                assert!(first.is_ok(), "k={k}: no fault, so it finishes");
                drop(first);
                check_done(&data, &new_id).await;
                assert!(k > 0, "the fault fired at least once");
                return;
            }
            assert!(first.is_err(), "k={k}: the fault surfaces");
            check_done(&data, &new_id).await;
        }
        panic!("the swap never ran without hitting the fault");
    }

    #[tokio::test]
    async fn swap_twice_is_noop() {
        let (_dir, data, _old, new_id) = staged().await;
        drop(Engine::open(&data).await.unwrap());
        check_done(&data, &new_id).await;
        check_done(&data, &new_id).await;
    }
}
