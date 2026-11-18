//! Backup settings, secrets and target checks.

use std::path::Path;
use std::sync::Arc;

use printdex_core::backup::secrets::{MemoryBackend, SecretBackend, Store};
use printdex_core::backup::{
    BackupConfig, ModelSchedule, Repo, RepoOptions, Schedule, Target, TargetConfig, TargetSecrets,
};
use printdex_core::{BackupError, CoreError, Engine};

async fn engine(data: &Path, os: Option<Arc<dyn SecretBackend>>) -> Engine {
    Engine::open(data)
        .await
        .unwrap()
        .with_secret_store(Arc::new(Store::with_backend(os)))
}

fn s3() -> Target {
    Target::S3 {
        endpoint: "https://s3.example.com".into(),
        bucket: "prints".into(),
        region: "eu-central-1".into(),
        key_id: "AKIDEXAMPLE".into(),
        prefix: "printdex".into(),
    }
}

const SECRET: &str = "wJalrXUtnFEMI-very-secret-key";
const PASSPHRASE: &str = "correct horse battery staple";

#[tokio::test]
async fn config_round_trips_without_secrets() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let lib = tmp.path().join("lib");
    std::fs::create_dir_all(&lib).unwrap();
    let os = Arc::new(MemoryBackend::default());
    let e = engine(&data, Some(os.clone())).await;
    let library = e.add_library(&lib, None).await.unwrap();
    let config = BackupConfig {
        targets: vec![TargetConfig {
            id: 0,
            target: s3(),
            encrypted: true,
        }],
        schedule: Schedule {
            models: ModelSchedule::Weekly,
            upload_limit_kib: Some(512),
        },
        libraries: vec![(library.id, true)],
    };
    let saved = e.configure_backup(config.clone()).await.unwrap();
    let id = saved.targets[0].id;
    assert_ne!(id, 0);
    assert!(saved.models_on(library.id));
    let secrets = TargetSecrets {
        secret: Some(SECRET.into()),
        passphrase: PASSPHRASE.into(),
    };
    assert!(e.set_target_secrets(id, &secrets, true));
    assert!(e.target_secrets_known(&saved.targets[0]));
    drop(e);

    // Reopened: the same settings, and the secrets from the OS store.
    let e = engine(&data, Some(os.clone())).await;
    let again = e.backup_config().await.unwrap();
    assert_eq!(again, saved);
    assert!(e.target_secrets_known(&again.targets[0]));
    drop(e);

    // No secret anywhere in the data folder.
    for entry in walk(&data) {
        let bytes = std::fs::read(&entry).unwrap();
        for s in [SECRET, PASSPHRASE] {
            assert!(
                !bytes.windows(s.len()).any(|w| w == s.as_bytes()),
                "{s} found in {}",
                entry.display()
            );
        }
    }
    assert!(os.values().contains(&SECRET.to_owned()));

    // Removing the target forgets its secrets.
    let e = engine(&data, Some(os.clone())).await;
    e.configure_backup(BackupConfig::default()).await.unwrap();
    assert!(os.values().is_empty());
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[tokio::test]
async fn missing_keyring_falls_back_to_session_secret() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let e = engine(&data, None).await;
    assert!(!e.secrets_persistent());
    let saved = e
        .configure_backup(BackupConfig {
            targets: vec![TargetConfig {
                id: 0,
                target: s3(),
                encrypted: false,
            }],
            ..BackupConfig::default()
        })
        .await
        .unwrap();
    let t = &saved.targets[0];
    assert!(!e.target_secrets_known(t));
    let remembered = e.set_target_secrets(
        t.id,
        &TargetSecrets {
            secret: Some(SECRET.into()),
            passphrase: String::new(),
        },
        true,
    );
    // Asked to remember, but nowhere to keep it: this session only.
    assert!(!remembered);
    assert!(e.target_secrets_known(t));
    drop(e);
    let e = engine(&data, None).await;
    assert!(!e.target_secrets_known(&e.backup_config().await.unwrap().targets[0]));
}

#[tokio::test]
async fn test_target_checks_folder_and_passphrase() {
    let tmp = tempfile::tempdir().unwrap();
    let e = engine(&tmp.path().join("data"), None).await;
    let folder = Target::Folder {
        path: tmp.path().join("backups"),
    };
    let none = TargetSecrets::default();
    // Reachable and empty.
    let check = e.test_target(&folder, &none).await.unwrap();
    assert!(!check.has_backup);
    // With a backup there: the right passphrase opens it, a wrong one says so.
    let right = TargetSecrets {
        secret: None,
        passphrase: "right".into(),
    };
    Repo::open_or_init(&folder, &right, &RepoOptions::default()).unwrap();
    assert!(e.test_target(&folder, &right).await.unwrap().has_backup);
    let err = e.test_target(&folder, &none).await.unwrap_err();
    assert!(
        matches!(err, CoreError::Backup(BackupError::WrongPassphrase)),
        "{err:?}"
    );
    // Nothing besides the repository was left behind.
    assert!(!tmp.path().join("backups/.printdex-probe").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn test_target_detects_read_only_folder() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let e = engine(&tmp.path().join("data"), None).await;
    let ro = tmp.path().join("ro");
    std::fs::create_dir_all(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("x"), b"x").is_ok() {
        // Running as root: permissions don't apply.
        return;
    }
    let err = e
        .test_target(
            &Target::Folder { path: ro.clone() },
            &TargetSecrets::default(),
        )
        .await
        .unwrap_err();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        matches!(err, CoreError::Backup(BackupError::ReadOnly)),
        "{err:?}"
    );
}

#[tokio::test]
async fn passphrase_change_keeps_snapshots() {
    let tmp = tempfile::tempdir().unwrap();
    let e = engine(&tmp.path().join("data"), None).await;
    let folder = Target::Folder {
        path: tmp.path().join("backups"),
    };
    let saved = e
        .configure_backup(BackupConfig {
            targets: vec![TargetConfig {
                id: 0,
                target: folder.clone(),
                encrypted: true,
            }],
            ..BackupConfig::default()
        })
        .await
        .unwrap();
    let id = saved.targets[0].id;
    let old = TargetSecrets {
        secret: None,
        passphrase: "old".into(),
    };
    e.set_target_secrets(id, &old, false);
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.stl"), b"solid a\nendsolid a\n").unwrap();
    let mut repo = Repo::open_or_init(&folder, &old, &RepoOptions::default()).unwrap();
    repo.backup(
        &printdex_core::backup::BackupJob {
            source: &src,
            as_path: "/x",
            filter: &printdex_core::backup::Globs::default(),
            tags: &[],
            host: "h",
        },
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .unwrap();
    drop(repo);

    e.change_passphrase(id, "new", false).await.unwrap();
    let new = TargetSecrets {
        secret: None,
        passphrase: "new".into(),
    };
    let repo = Repo::open(&folder, &new, &RepoOptions::default()).unwrap();
    assert_eq!(repo.snapshots(&Default::default()).unwrap().len(), 1);
    assert!(Repo::open(&folder, &old, &RepoOptions::default()).is_err());
    // Turning encryption off is a change to the empty passphrase.
    e.change_passphrase(id, "", false).await.unwrap();
    let cfg = e.backup_config().await.unwrap();
    assert!(!cfg.targets[0].encrypted);
    assert!(Repo::open(&folder, &TargetSecrets::default(), &RepoOptions::default()).is_ok());
}
