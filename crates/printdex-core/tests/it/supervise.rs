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

// Real time throughout: paused time auto-advances whenever the runtime waits
// on SQLite's own thread, so the backoffs (1 s + 5 s) could be skipped before
// the task ran, or the database pool's acquire timeout could fire. The stop
// itself is the signal; a stopped supervisor has returned, so no run follows.
#[tokio::test(flavor = "multi_thread")]
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
    let task = next(&mut rx, Duration::from_secs(30), |e| match e {
        EngineEvent::TaskStopped(t) => Some(t),
        _ => None,
    })
    .await;
    assert_eq!(task, Some(TaskName::Thumbnails), "stopped after 3 panics");
    // Each run's workers may all panic; what matters is no further run.
    let fired = guard.fired();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(guard.fired(), fired, "no restart after the third panic");
    assert!(stopped(&mut rx).is_empty(), "stopped once");
    assert_eq!(engine.search("hook", 10).await.unwrap().len(), 1);
}

/// After the armed panic fired, touches a new part every 500 ms until a
/// rescan finishes: whenever the restarted watcher is ready, the next touch
/// is seen. No fixed sleep to outlast the restart backoff.
async fn rescan_after_restart(f: &Fixture, rx: &mut broadcast::Receiver<EngineEvent>) {
    // A rescan from before the panic does not count.
    while rx.try_recv().is_ok() {}
    let found = tokio::time::timeout(Duration::from_secs(15), async {
        for n in 2.. {
            touch(&f.root, &format!("Hook/hook-v{n}.stl"));
            if next(rx, Duration::from_millis(500), finished)
                .await
                .is_some()
            {
                return;
            }
        }
    })
    .await;
    assert!(found.is_ok(), "a rescan after the restart");
}

async fn panicked(guard: &faults::FaultGuard) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while guard.fired() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the monitor panicked");
}

/// The outer loop that starts the per-library workers panics, and is
/// restarted with its workers.
#[tokio::test(flavor = "multi_thread")]
async fn monitor_restarts_after_panic() {
    let f = Fixture::new(ChangeMode::Watch, LibraryKind::Local, Arc::new(RealFs)).await;
    let mut rx = f.engine.subscribe();
    let guard = faults::arm_panic_at(TaskName::Monitor, "supervise", 1);
    let monitor = f.engine.start_monitor_with(fast());
    panicked(&guard).await;
    rescan_after_restart(&f, &mut rx).await;
    assert_eq!(guard.fired(), 1);
    assert!(stopped(&mut rx).is_empty());
    monitor.stop().await;
}

/// One library's worker panics and is restarted alone.
#[tokio::test(flavor = "multi_thread")]
async fn library_worker_restarts_after_panic() {
    let f = Fixture::new(ChangeMode::Watch, LibraryKind::Local, Arc::new(RealFs)).await;
    let mut rx = f.engine.subscribe();
    let guard = faults::arm_panic_at(TaskName::Monitor, "library", 1);
    let monitor = f.engine.start_monitor_with(fast());
    panicked(&guard).await;
    rescan_after_restart(&f, &mut rx).await;
    assert_eq!(guard.fired(), 1);
    assert!(stopped(&mut rx).is_empty());
    monitor.stop().await;
}
