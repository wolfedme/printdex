//! The library monitor: watching, timers and offline detection.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use printdex_core::db::Db;
use printdex_core::fs::{ChaosFs, Fs, RealFs};
use printdex_core::{
    ChangeMode, Engine, EngineEvent, Library, LibraryKind, MonitorConfig, ScanSummary,
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, rel.as_bytes()).unwrap();
}

fn fast() -> MonitorConfig {
    MonitorConfig {
        probe_every: Duration::from_millis(100),
        debounce: Duration::from_millis(300),
        debounce_cap: Duration::from_secs(3),
        minute: Duration::from_millis(50),
        busy_retry: Duration::from_millis(100),
        redetect: false,
        ..MonitorConfig::default()
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
    engine: Engine,
    lib: Library,
}

impl Fixture {
    async fn new(mode: ChangeMode, kind: LibraryKind, fs: Arc<dyn Fs>) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        touch(&root, "Hook/hook.stl");
        touch(&root, "Bin/bin.stl");
        let data = dir.path().join("data");
        let engine = Engine::open_with_fs(&data, &data.join("cache"), fs)
            .await
            .unwrap();
        let lib = engine.add_library(&root, None).await.unwrap();
        engine
            .rescan(lib.id, CancellationToken::new())
            .await
            .unwrap();
        let db = Db::open(&data.join("printdex.db")).await.unwrap();
        db.set_change_mode(lib.id, mode).await.unwrap();
        db.set_library_kind(lib.id, kind).await.unwrap();
        let lib = engine.libraries().await.unwrap().remove(0);
        Fixture {
            _dir: dir,
            root,
            data,
            engine,
            lib,
        }
    }
}

/// The next event `pick` accepts, or `None` after `within`.
async fn next<T>(
    rx: &mut broadcast::Receiver<EngineEvent>,
    within: Duration,
    mut pick: impl FnMut(EngineEvent) -> Option<T>,
) -> Option<T> {
    tokio::time::timeout(within, async {
        loop {
            match rx.recv().await {
                Ok(e) => {
                    if let Some(t) = pick(e) {
                        return t;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => std::future::pending().await,
            }
        }
    })
    .await
    .ok()
}

fn finished(e: EngineEvent) -> Option<ScanSummary> {
    match e {
        EngineEvent::ScanFinished { summary, .. } => Some(summary),
        _ => None,
    }
}

fn started(e: EngineEvent) -> Option<()> {
    matches!(e, EngineEvent::ScanStarted { .. }).then_some(())
}

async fn wait_online(rx: &mut broadcast::Receiver<EngineEvent>) {
    next(rx, Duration::from_secs(5), |e| {
        matches!(e, EngineEvent::LibraryOnline(_)).then_some(())
    })
    .await
    .expect("library online");
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_event_triggers_subtree_rescan_after_debounce() {
    let f = Fixture::new(ChangeMode::Watch, LibraryKind::Local, Arc::new(RealFs)).await;
    let mut rx = f.engine.subscribe();
    let monitor = f.engine.start_monitor_with(fast());
    wait_online(&mut rx).await;
    // Give the watcher a moment to register its watches.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let wrote = Instant::now();
    touch(&f.root, "Hook/hook-v2.stl");
    let summary = next(&mut rx, Duration::from_secs(10), finished)
        .await
        .expect("a rescan");
    assert!(wrote.elapsed() >= Duration::from_millis(300), "debounced");
    assert!(summary.partial, "only the changed path is walked");
    assert_eq!(summary.files_seen, 1);
    assert_eq!(summary.fingerprinted, 1);
    assert_eq!(summary.models_missing, 0, "the rest of the library stays");
    let hook = f
        .engine
        .search("hook", 10)
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    assert_eq!(hook.part_count, 2);
    assert_eq!(f.engine.search("bin", 10).await.unwrap().len(), 1);
    monitor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn deleted_folder_seen_by_watcher_goes_missing() {
    let f = Fixture::new(ChangeMode::Watch, LibraryKind::Local, Arc::new(RealFs)).await;
    let mut rx = f.engine.subscribe();
    let monitor = f.engine.start_monitor_with(fast());
    wait_online(&mut rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    fs::remove_dir_all(f.root.join("Bin")).unwrap();
    let summary = next(&mut rx, Duration::from_secs(10), finished)
        .await
        .expect("a rescan");
    assert!(summary.partial);
    assert_eq!(summary.models_missing, 1);
    assert!(f.engine.search("bin", 10).await.unwrap().is_empty());
    assert_eq!(f.engine.search("hook", 10).await.unwrap().len(), 1);
    monitor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn many_events_fall_back_to_full_rescan() {
    let f = Fixture::new(ChangeMode::Watch, LibraryKind::Local, Arc::new(RealFs)).await;
    let mut rx = f.engine.subscribe();
    let monitor = f.engine.start_monitor_with(MonitorConfig {
        max_paths: 3,
        ..fast()
    });
    wait_online(&mut rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    for i in 0..6 {
        touch(&f.root, &format!("New{i}/part.stl"));
    }
    let summary = next(&mut rx, Duration::from_secs(10), finished)
        .await
        .expect("a rescan");
    assert!(!summary.partial, "too many changes: the whole library");
    assert_eq!(summary.models_added, 6);
    monitor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sidecar_writes_do_not_trigger_rescans() {
    let f = Fixture::new(ChangeMode::Watch, LibraryKind::Local, Arc::new(RealFs)).await;
    let mut rx = f.engine.subscribe();
    let monitor = f.engine.start_monitor_with(fast());
    wait_online(&mut rx).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    fs::write(f.root.join("Hook/.printdex.toml"), "x = 1\n").unwrap();
    assert!(
        next(&mut rx, Duration::from_millis(1200), started)
            .await
            .is_none()
    );
    monitor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn network_library_rescans_on_timer() {
    // Auto on a share: every 15 "minutes" (50 ms each here).
    let f = Fixture::new(ChangeMode::Auto, LibraryKind::Network, Arc::new(RealFs)).await;
    let mut rx = f.engine.subscribe();
    let monitor = f.engine.start_monitor_with(fast());
    let t = Instant::now();
    let summary = next(&mut rx, Duration::from_secs(10), finished)
        .await
        .expect("a timed rescan");
    assert!(!summary.partial);
    assert!(
        t.elapsed() >= Duration::from_millis(700),
        "waited for the timer"
    );
    monitor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_mode_never_rescans() {
    let f = Fixture::new(ChangeMode::Manual, LibraryKind::Local, Arc::new(RealFs)).await;
    let mut rx = f.engine.subscribe();
    let monitor = f.engine.start_monitor_with(fast());
    wait_online(&mut rx).await;
    touch(&f.root, "Hook/hook-v2.stl");
    assert!(
        next(&mut rx, Duration::from_millis(1500), started)
            .await
            .is_none()
    );
    monitor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn offline_then_online_triggers_rescan() {
    let chaos = Arc::new(ChaosFs::new());
    let f = Fixture::new(ChangeMode::Manual, LibraryKind::Network, chaos.clone()).await;
    let mut rx = f.engine.subscribe();
    chaos.unmount(&f.root);
    let monitor = f.engine.start_monitor_with(fast());
    next(&mut rx, Duration::from_secs(5), |e| {
        matches!(e, EngineEvent::LibraryOffline(_)).then_some(())
    })
    .await
    .expect("offline");
    chaos.remount(&f.root);
    wait_online(&mut rx).await;
    let summary = next(&mut rx, Duration::from_secs(5), finished)
        .await
        .expect("rescan on return");
    assert!(!summary.partial);
    assert_eq!(summary.models_missing, 0);
    assert_eq!(f.engine.search("", 10).await.unwrap().len(), 2);
    monitor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_failure_falls_back_to_timer() {
    let f = Fixture::new(ChangeMode::Watch, LibraryKind::Local, Arc::new(RealFs)).await;
    let mut rx = f.engine.subscribe();
    let monitor = f.engine.start_monitor_with(MonitorConfig {
        fail_watch: true,
        ..fast()
    });
    let summary = next(&mut rx, Duration::from_secs(10), finished)
        .await
        .expect("a timed rescan instead of watching");
    assert!(!summary.partial);
    monitor.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_the_mode_restarts_the_library() {
    let f = Fixture::new(ChangeMode::Manual, LibraryKind::Local, Arc::new(RealFs)).await;
    let mut rx = f.engine.subscribe();
    let monitor = f.engine.start_monitor_with(fast());
    wait_online(&mut rx).await;
    f.engine
        .set_change_mode(f.lib.id, ChangeMode::Every(2))
        .await
        .unwrap();
    let summary = next(&mut rx, Duration::from_secs(10), finished)
        .await
        .expect("the new timer runs");
    assert!(!summary.partial);
    let _ = &f.data;
    monitor.stop().await;
}
