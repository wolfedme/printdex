//! The engine on a slow, flaky or vanishing filesystem.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use printdex_core::fs::ChaosFs;
use printdex_core::{CoreError, Engine};
use tokio_util::sync::CancellationToken;

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, rel.as_bytes()).unwrap();
}

fn many(root: &Path, n: usize) {
    for i in 0..n {
        touch(root, &format!("M{}/p{i}.stl", i % (n / 4).max(1)));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn scan_lock_file_blocks_second_process() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    many(&root, 400);
    let data = dir.path().join("data");
    let slow = Arc::new(ChaosFs::new().with_latency(Duration::from_millis(2)));
    let a = Engine::open_with_fs(&data, &data.join("cache"), slow)
        .await
        .unwrap();
    // A second engine on the same data folder stands in for the CLI.
    let b = Engine::open(&data).await.unwrap();
    let lib = a.add_library(&root, None).await.unwrap();
    let mut rx = a.subscribe();
    let first = tokio::spawn({
        let a = a.clone();
        async move { a.rescan(lib.id, CancellationToken::new()).await }
    });
    loop {
        if let printdex_core::EngineEvent::ScanStarted { .. } = rx.recv().await.unwrap() {
            break;
        }
    }
    assert!(matches!(
        b.rescan(lib.id, CancellationToken::new()).await,
        Err(CoreError::AlreadyScanning(_))
    ));
    first.await.unwrap().unwrap();
    b.rescan(lib.id, CancellationToken::new()).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_rescan_future_stops_walk() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    many(&root, 2_000);
    let data = dir.path().join("data");
    let fs = Arc::new(ChaosFs::new().with_latency(Duration::from_millis(3)));
    let engine = Engine::open_with_fs(&data, &data.join("cache"), fs.clone())
        .await
        .unwrap();
    let lib = engine.add_library(&root, None).await.unwrap();
    let r = tokio::time::timeout(
        Duration::from_millis(60),
        engine.rescan(lib.id, CancellationToken::new()),
    )
    .await;
    assert!(r.is_err(), "the scan should still be walking");
    tokio::time::sleep(Duration::from_millis(200)).await;
    let after_drop = fs.calls();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(fs.calls(), after_drop, "the walk kept going");
    // And the library can be scanned again right away.
    engine
        .rescan(lib.id, CancellationToken::new())
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_share_scans_completely() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    many(&root, 200);
    let data = dir.path().join("data");
    let fs = Arc::new(ChaosFs::new().with_latency(Duration::from_millis(1)));
    let engine = Engine::open_with_fs(&data, &data.join("cache"), fs)
        .await
        .unwrap();
    let lib = engine.add_library(&root, None).await.unwrap();
    let s = engine
        .rescan(lib.id, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(s.files_seen, 200);
    assert_eq!(s.models_added, 50);
}

#[cfg(unix)]
#[tokio::test]
async fn symlink_to_offline_target_reports_offline() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("nas");
    std::os::unix::fs::symlink(dir.path().join("unmounted"), &link).unwrap();
    let engine = Engine::open(&dir.path().join("data")).await.unwrap();
    assert!(matches!(
        engine.add_library(&link, None).await,
        Err(CoreError::LibraryOffline(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn vanishing_mount_mid_walk_marks_nothing_missing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    many(&root, 800);
    let data = dir.path().join("data");
    let fs = Arc::new(ChaosFs::new().with_latency(Duration::from_millis(1)));
    let engine = Engine::open_with_fs(&data, &data.join("cache"), fs.clone())
        .await
        .unwrap();
    let lib = engine.add_library(&root, None).await.unwrap();
    engine
        .rescan(lib.id, CancellationToken::new())
        .await
        .unwrap();
    let before = engine.stats().await.unwrap();

    // The share drops while the second scan walks.
    let scan = tokio::spawn({
        let engine = engine.clone();
        async move { engine.rescan(lib.id, CancellationToken::new()).await }
    });
    tokio::time::sleep(Duration::from_millis(40)).await;
    fs.unmount(&root);
    let r = scan.await.unwrap();
    assert!(
        matches!(r, Err(CoreError::LibraryOffline(_))),
        "expected offline, got {r:?}"
    );
    let after = engine.stats().await.unwrap();
    assert_eq!(after.models, before.models);
    assert_eq!(after.missing_models, 0);

    // Back online: everything is still there.
    fs.remount(&root);
    let s = engine
        .rescan(lib.id, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!((s.models_missing, s.models_added), (0, 0));
}

#[tokio::test(flavor = "multi_thread")]
async fn flaky_reads_mark_nothing_missing() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    many(&root, 400);
    let data = dir.path().join("data");
    let engine = Engine::open(&data).await.unwrap();
    let lib = engine.add_library(&root, None).await.unwrap();
    engine
        .rescan(lib.id, CancellationToken::new())
        .await
        .unwrap();
    let before = engine.stats().await.unwrap();
    drop(engine);

    // The same data, now through a share that drops one call in 13.
    let flaky = Arc::new(ChaosFs::new().failing_every(13));
    let engine = Engine::open_with_fs(&data, &data.join("cache"), flaky)
        .await
        .unwrap();
    for _ in 0..3 {
        match engine.rescan(lib.id, CancellationToken::new()).await {
            Ok(s) => assert_eq!(s.models_missing, 0, "{s:?}"),
            Err(CoreError::LibraryOffline(_)) => {}
            Err(e) => panic!("{e}"),
        }
        let now = engine.stats().await.unwrap();
        assert_eq!(now.missing_models, 0);
        assert_eq!(now.models, before.models);
    }
}

#[tokio::test]
async fn offline_library_does_not_age() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    many(&root, 40);
    let engine = Engine::open(&dir.path().join("data")).await.unwrap();
    let lib = engine.add_library(&root, None).await.unwrap();
    engine
        .rescan(lib.id, CancellationToken::new())
        .await
        .unwrap();
    // Unplugged: every scan fails as offline and changes nothing.
    fs::rename(&root, dir.path().join("unplugged")).unwrap();
    for _ in 0..3 {
        assert!(matches!(
            engine.rescan(lib.id, CancellationToken::new()).await,
            Err(CoreError::LibraryOffline(_))
        ));
    }
    let s = engine.stats().await.unwrap();
    assert_eq!((s.models, s.missing_models), (10, 0));
}
