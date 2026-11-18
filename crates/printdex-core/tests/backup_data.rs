//! Data snapshots: the database, photos and settings files.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use printdex_core::backup::secrets::Store;
use printdex_core::backup::{
    BackupConfig, BackupKind, BackupScope, DATA_TAG, Repo, RepoOptions, SnapshotFilter, Target,
    TargetConfig, TargetSecrets,
};
use printdex_core::{
    AttemptResult, BackupError, CoreError, Engine, EngineEvent, NewAttempt, PrintSettings,
};
use tokio_util::sync::CancellationToken;

struct Fx {
    dir: tempfile::TempDir,
    engine: Engine,
    target: Target,
}

const SECRET_PASS: &str = "zebra-violin-orbit-42";

impl Fx {
    /// An engine with one scanned library, a folder target (encrypted with
    /// SECRET_PASS) and a settings file.
    async fn new() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib/Bin");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("bin.stl"), b"solid bin\nendsolid bin\n").unwrap();
        let engine = Engine::open_with_cache(&dir.path().join("data"), &dir.path().join("cache"))
            .await
            .unwrap()
            .with_secret_store(Arc::new(Store::with_backend(None)));
        let l = engine
            .add_library(&dir.path().join("lib"), None)
            .await
            .unwrap();
        engine.rescan(l.id, CancellationToken::new()).await.unwrap();
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
                ..BackupConfig::default()
            })
            .await
            .unwrap();
        engine.set_target_secrets(
            cfg.targets[0].id,
            &TargetSecrets {
                secret: None,
                passphrase: SECRET_PASS.into(),
            },
            false,
        );
        let settings = dir.path().join("config/settings.toml");
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        std::fs::write(&settings, "mode = \"dark\"\n").unwrap();
        engine.set_backup_files(vec![settings, dir.path().join("config/apps.toml")]);
        Fx {
            dir,
            engine,
            target,
        }
    }

    fn secrets(&self) -> TargetSecrets {
        TargetSecrets {
            secret: None,
            passphrase: SECRET_PASS.into(),
        }
    }

    /// Restores the newest data snapshot into a fresh folder.
    fn restore_latest(&self) -> PathBuf {
        let mut repo = Repo::open(&self.target, &self.secrets(), &RepoOptions::default()).unwrap();
        let snaps = repo
            .snapshots(&SnapshotFilter {
                host: None,
                tag: Some(DATA_TAG.into()),
            })
            .unwrap();
        let out = self.dir.path().join(format!("out-{}", snaps.len()));
        repo.restore(
            &snaps.last().unwrap().id,
            "",
            &out,
            &CancellationToken::new(),
            None,
        )
        .unwrap();
        out
    }

    async fn model(&self) -> printdex_core::ModelId {
        self.engine.search("bin", 1).await.unwrap()[0].id
    }
}

fn files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for e in walkdir(dir) {
        out.push(
            e.strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/"),
        );
    }
    out.sort();
    out
}

fn walkdir(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            out.extend(walkdir(&p));
        } else {
            out.push(p);
        }
    }
    out
}

fn png(path: &Path) {
    let img = image::RgbImage::from_pixel(8, 8, image::Rgb([200, 30, 30]));
    img.save(path).unwrap();
}

fn attempt() -> NewAttempt {
    NewAttempt {
        printed_at: 1_763_000_000_000,
        result: AttemptResult::Success,
        settings: PrintSettings::default(),
        duration_min: None,
        notes: String::new(),
        parts: vec![],
    }
}

async fn integrity_ok(db: &Path) {
    use sqlx::Connection;
    let mut c = sqlx::SqliteConnection::connect(&format!("sqlite://{}", db.display()))
        .await
        .unwrap();
    let r: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&mut c)
        .await
        .unwrap();
    assert_eq!(r, "ok");
}

#[tokio::test]
async fn data_snapshot_has_db_photos_settings_not_caches() {
    let fx = Fx::new().await;
    let m = fx.model().await;
    fx.engine.add_tag(m, "workshop").await.unwrap();
    let a = fx.engine.log_attempt(m, &attempt()).await.unwrap();
    let photo = fx.dir.path().join("photo.png");
    png(&photo);
    fx.engine.add_photo(a, &photo).await.unwrap();
    // A thumbnail cache exists but must not be backed up.
    std::fs::create_dir_all(fx.dir.path().join("cache/thumbs")).unwrap();

    let mut events = fx.engine.subscribe();
    let runs = fx.engine.backup_now(BackupScope::Data).await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].kind, BackupKind::Data);
    let snap = runs[0].snapshot.clone().unwrap();
    assert_eq!(snap.tags, [DATA_TAG]);
    assert_eq!(snap.host, fx.engine_install());

    let mut started = false;
    let mut finished = false;
    while let Ok(e) = events.try_recv() {
        match e {
            EngineEvent::BackupStarted { kind } if kind == BackupKind::Data => started = true,
            EngineEvent::BackupFinished { kind, .. } if kind == BackupKind::Data => finished = true,
            _ => {}
        }
    }
    assert!(started && finished);

    let out = fx.restore_latest();
    let names = files(&out);
    assert!(names.contains(&"printdex.db".to_owned()), "{names:?}");
    assert!(names.contains(&"settings.toml".to_owned()), "{names:?}");
    assert_eq!(
        names.iter().filter(|n| n.starts_with("photos/")).count(),
        1,
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .all(|n| !n.contains("cache") && !n.contains("-wal") && !n.contains("lock")),
        "{names:?}"
    );
    integrity_ok(&out.join("printdex.db")).await;
    // The copy holds the tag.
    let restored = Engine::open_with_cache(&out, &fx.dir.path().join("cache2"))
        .await
        .unwrap();
    assert_eq!(restored.search("workshop", 5).await.unwrap().len(), 1);
    // The history shows the run.
    let runs = fx.engine.backup_runs(10).await.unwrap();
    assert_eq!(runs[0].result, "ok");
}

impl Fx {
    fn engine_install(&self) -> String {
        self.engine.install_id().to_string()
    }
}

#[tokio::test]
async fn db_copy_is_consistent_during_writes() {
    let fx = Fx::new().await;
    let m = fx.model().await;
    let writer = {
        let engine = fx.engine.clone();
        tokio::spawn(async move {
            for i in 0..200 {
                engine.add_tag(m, &format!("tag{i}")).await.unwrap();
            }
        })
    };
    fx.engine.backup_now(BackupScope::Data).await.unwrap();
    writer.await.unwrap();
    fx.engine.backup_now(BackupScope::Data).await.unwrap();
    let out = fx.restore_latest();
    integrity_ok(&out.join("printdex.db")).await;
}

#[tokio::test]
async fn interrupted_backup_retries_cleanly() {
    let fx = Fx::new().await;
    // A big settings file so the backup takes a moment.
    let big = fx.dir.path().join("config/big.bin");
    let mut rng = fastrand::Rng::with_seed(3);
    std::fs::write(
        &big,
        (0..30_000_000).map(|_| rng.u8(..)).collect::<Vec<u8>>(),
    )
    .unwrap();
    fx.engine.set_backup_files(vec![big]);
    let mut events = fx.engine.subscribe();
    let engine = fx.engine.clone();
    let canceller = tokio::spawn(async move {
        while let Ok(e) = events.recv().await {
            if let EngineEvent::BackupProgress { done, .. } = e
                && done > 1_000_000
            {
                engine.cancel_backup();
                break;
            }
        }
    });
    let err = fx.engine.backup_now(BackupScope::Data).await.unwrap_err();
    assert!(matches!(err, CoreError::Cancelled), "{err:?}");
    canceller.await.unwrap();
    assert_eq!(
        fx.engine.backup_runs(1).await.unwrap()[0].result,
        "cancelled"
    );

    // The next run completes and the repository checks clean.
    fx.engine.backup_now(BackupScope::Data).await.unwrap();
    let mut repo = Repo::open(&fx.target, &fx.secrets(), &RepoOptions::default()).unwrap();
    repo.check(100.0).unwrap();
    assert_eq!(repo.snapshots(&SnapshotFilter::default()).unwrap().len(), 1);
}

#[tokio::test]
async fn no_secret_in_snapshot() {
    let fx = Fx::new().await;
    fx.engine.backup_now(BackupScope::Data).await.unwrap();
    let out = fx.restore_latest();
    for f in walkdir(&out) {
        let bytes = std::fs::read(&f).unwrap();
        assert!(
            !bytes
                .windows(SECRET_PASS.len())
                .any(|w| w == SECRET_PASS.as_bytes()),
            "passphrase in {}",
            f.display()
        );
    }
}

#[tokio::test]
async fn wrong_passphrase_fails_without_writing() {
    let fx = Fx::new().await;
    fx.engine.backup_now(BackupScope::Data).await.unwrap();
    let id = fx.engine.backup_config().await.unwrap().targets[0].id;
    fx.engine.set_target_secrets(
        id,
        &TargetSecrets {
            secret: None,
            passphrase: "nope".into(),
        },
        false,
    );
    let before = std::fs::read_dir(fx.dir.path().join("backups/snapshots"))
        .unwrap()
        .count();
    let mut events = fx.engine.subscribe();
    let err = fx.engine.backup_now(BackupScope::Data).await.unwrap_err();
    assert!(
        matches!(err, CoreError::Backup(BackupError::WrongPassphrase)),
        "{err:?}"
    );
    let mut failed = false;
    while let Ok(e) = events.try_recv() {
        failed |= matches!(e, EngineEvent::BackupFailed { .. });
    }
    assert!(failed);
    let after = std::fs::read_dir(fx.dir.path().join("backups/snapshots"))
        .unwrap()
        .count();
    assert_eq!(before, after);
}

#[tokio::test]
async fn no_target_means_no_backup() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(&dir.path().join("data")).await.unwrap();
    let err = engine.backup_now(BackupScope::Data).await.unwrap_err();
    assert!(
        matches!(err, CoreError::Backup(BackupError::NotConfigured)),
        "{err:?}"
    );
}
