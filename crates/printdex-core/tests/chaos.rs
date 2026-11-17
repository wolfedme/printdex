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
