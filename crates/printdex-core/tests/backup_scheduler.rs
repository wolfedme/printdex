//! The backup scheduler and retention across installs.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use printdex_core::backup::secrets::Store;
use printdex_core::backup::{
    Attention, BackupConfig, BackupKind, BackupScope, DATA_TAG, Repo, RepoOptions, SchedulerConfig,
    SnapshotFilter, Target, TargetConfig, TargetSecrets,
};
use printdex_core::{Engine, EngineEvent};

async fn engine(data: &Path, target: &Target, encrypted: bool) -> Engine {
    let e = Engine::open(data)
        .await
        .unwrap()
        .with_secret_store(Arc::new(Store::with_backend(None)));
    e.configure_backup(BackupConfig {
        targets: vec![TargetConfig {
            id: 0,
            target: target.clone(),
            encrypted,
        }],
        ..BackupConfig::default()
    })
    .await
    .unwrap();
    e
}

#[tokio::test]
async fn retention_is_per_host_and_tag() {
    let tmp = tempfile::tempdir().unwrap();
    let target = Target::Folder {
        path: tmp.path().join("backups"),
    };
    let a = engine(&tmp.path().join("a"), &target, false).await;
    let b = engine(&tmp.path().join("b"), &target, false).await;
    for _ in 0..3 {
        a.backup_now(BackupScope::Data).await.unwrap();
        b.backup_now(BackupScope::Data).await.unwrap();
    }
    // Same day: each install keeps its newest snapshot plus its oldest
    // (restic keeps the oldest while fewer than 7 days exist), and neither
    // thins the other's.
    let repo = Repo::open(&target, &TargetSecrets::default(), &RepoOptions::default()).unwrap();
    for e in [&a, &b] {
        let n = repo
            .snapshots(&SnapshotFilter {
                host: Some(e.install_id().to_string()),
                tag: Some(DATA_TAG.into()),
            })
            .unwrap()
            .len();
        assert_eq!(n, 2);
    }
}

fn fast() -> SchedulerConfig {
    SchedulerConfig {
        tick: Duration::from_millis(100),
        launch_grace: 0,
    }
}

#[tokio::test]
async fn scheduler_backs_up_data_when_due() {
    let tmp = tempfile::tempdir().unwrap();
    let target = Target::Folder {
        path: tmp.path().join("backups"),
    };
    let e = engine(&tmp.path().join("data"), &target, false).await;
    let mut events = e.subscribe();
    let sched = e.start_backup_scheduler_with(fast());
    let done = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(EngineEvent::BackupFinished {
                kind: BackupKind::Data,
                snapshot: Some(_),
            }) = events.recv().await
            {
                break;
            }
        }
    })
    .await;
    drop(sched);
    assert!(done.is_ok(), "no data backup within 30 s");
    assert_eq!(
        e.backup_runs(5)
            .await
            .unwrap()
            .iter()
            .filter(|r| r.kind == "data")
            .count(),
        1
    );
}

#[tokio::test]
async fn missing_passphrase_needs_attention() {
    let tmp = tempfile::tempdir().unwrap();
    let target = Target::Folder {
        path: tmp.path().join("backups"),
    };
    // Encrypted, but this session has no passphrase.
    let e = engine(&tmp.path().join("data"), &target, true).await;
    let mut events = e.subscribe();
    let sched = e.start_backup_scheduler_with(fast());
    let got = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(EngineEvent::BackupNeedsAttention(a)) = events.recv().await {
                return a;
            }
        }
    })
    .await
    .unwrap();
    drop(sched);
    assert_eq!(got, [Attention::PassphraseNeeded]);
    assert_eq!(e.backup_attention(), [Attention::PassphraseNeeded]);
    assert!(
        e.backup_runs(5).await.unwrap().is_empty(),
        "nothing was tried"
    );
}
