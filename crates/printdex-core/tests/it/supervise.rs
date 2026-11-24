//! Supervised background tasks: a panic restarts the task, repeated panics
//! stop it with `TaskStopped`.

use std::fs;
use std::sync::Arc;
use std::time::Duration;

use printdex_core::fs::RealFs;
use printdex_core::fsx::faults;
use printdex_core::{ChangeMode, Engine, EngineEvent, LibraryKind, TaskName, ThumbTarget};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::monitor::{Fixture, fast, finished, next, touch};

/// Every `TaskStopped` waiting in `rx`.
fn stopped(rx: &mut broadcast::Receiver<EngineEvent>) -> Vec<TaskName> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(EngineEvent::TaskStopped(t)) => out.push(t),
            Ok(_) | Err(broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(_) => return out,
        }
    }
}

#[tokio::test]
async fn sidecar_writer_survives_a_panic() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    touch(&root, "Bin/a.stl");
    let engine = Engine::open(&dir.path().join("data")).await.unwrap();
    let lib = engine.add_library(&root, None).await.unwrap();
    engine
        .rescan(lib.id, CancellationToken::new())
        .await
        .unwrap();
    let bin = engine.search("Bin", 10).await.unwrap()[0].id;
    let mut rx = engine.subscribe();
    let guard = faults::arm_panic(TaskName::SidecarWriter, 1);
    engine.add_tag(bin, "Gridfinity").await.unwrap();
    engine.flush_sidecars().await;
    assert_eq!(guard.fired(), 1, "the writer panicked once");
    let text = fs::read_to_string(root.join("Bin/.printdex.toml")).expect("sidecar written");
    assert!(text.contains("Gridfinity"), "{text}");
    assert!(stopped(&mut rx).is_empty(), "one panic does not stop it");
}

// Paused time only around the backoffs: under auto-advance the database
// pool's acquire timeout fires while SQLite works on its own thread.
#[tokio::test]
async fn gives_up_after_three_panics() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    touch(&root, "Hook/hook.stl");
    let guard = faults::arm_panic(TaskName::Thumbnails, usize::MAX);
    let engine = Engine::open(&dir.path().join("data")).await.unwrap();
    let mut rx = engine.subscribe();
    let lib = engine.add_library(&root, None).await.unwrap();
    engine
        .rescan(lib.id, CancellationToken::new())
        .await
        .unwrap();
    let hook = engine.search("hook", 10).await.unwrap()[0].id;
    engine
        .thumbnails()
        .request_visible([ThumbTarget::Model(hook)]);
    tokio::time::pause();
    // Past the backoffs (1 s + 5 s, and 30 s were there a fourth run).
    tokio::time::sleep(Duration::from_secs(40)).await;
    assert_eq!(stopped(&mut rx), [TaskName::Thumbnails]);
    // Each run's workers may all panic; what matters is no fourth run.
    let fired = guard.fired();
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(guard.fired(), fired, "no restart after the third panic");
    assert!(stopped(&mut rx).is_empty(), "stopped once");
    tokio::time::resume();
    assert_eq!(engine.search("hook", 10).await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn monitor_restarts_after_panic() {
    let f = Fixture::new(ChangeMode::Watch, LibraryKind::Local, Arc::new(RealFs)).await;
    let mut rx = f.engine.subscribe();
    let guard = faults::arm_panic(TaskName::Monitor, 1);
    let monitor = f.engine.start_monitor_with(fast());
    tokio::time::timeout(Duration::from_secs(5), async {
        while guard.fired() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the monitor panicked");
    // Past the first restart backoff (1 s), and the watcher registered.
    tokio::time::sleep(Duration::from_millis(1700)).await;
    touch(&f.root, "Hook/hook-v2.stl");
    let summary = next(&mut rx, Duration::from_secs(10), finished)
        .await
        .expect("a rescan after the restart");
    assert_eq!(summary.files_seen, 1);
    assert!(stopped(&mut rx).is_empty());
    monitor.stop().await;
}
