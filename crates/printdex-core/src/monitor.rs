//! The library monitor: notices changes and offline shares without the
//! user pressing Refresh.
//!
//! Per library it checks the root every [`MonitorConfig::probe_every`]
//! (`LibraryOnline` / `LibraryOffline` events; coming back online rescans),
//! and depending on the library's [`ChangeMode`]:
//!
//! - **Watch**: filesystem events are debounced, collected into the paths
//!   they touch, and rescanned as a subtree scan. Too many paths, or a
//!   watcher that dropped events, rescans the whole library. A watcher that
//!   cannot start (inotify limit, unsupported filesystem) falls back to a
//!   timer.
//! - **Every N minutes**: a full (incremental) rescan on a timer.
//! - **Manual**: only the online checks.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::db::{ChangeMode, Library, LibraryKind};
use crate::engine::{Engine, LibraryStatus, folder_status};
use crate::error::CoreError;
use crate::events::EngineEvent;
use crate::ids::LibraryId;
use crate::supervise::{TaskName, spawn_supervised};
use crate::walk::is_sidecar_name;

/// Timings and limits of the monitor. The defaults are the product's;
/// tests shrink them.
#[derive(Clone, Debug)]
pub struct MonitorConfig {
    /// How often each library root is checked.
    pub probe_every: Duration,
    /// Quiet time after the last filesystem event before rescanning.
    pub debounce: Duration,
    /// Longest a burst of events can postpone a rescan.
    pub debounce_cap: Duration,
    /// More changed paths than this rescans the whole library.
    pub max_paths: usize,
    /// Length of a "minute" for [`ChangeMode::Every`] (tests shorten it).
    pub minute: Duration,
    /// Wait before retrying when a scan of the library is already running.
    pub busy_retry: Duration,
    /// Detect each library's volume kind again when the monitor starts.
    pub redetect: bool,
    /// Tests: pretend the watcher cannot start.
    #[doc(hidden)]
    pub fail_watch: bool,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        MonitorConfig {
            probe_every: Duration::from_secs(30),
            debounce: Duration::from_secs(2),
            debounce_cap: Duration::from_secs(20),
            max_paths: 500,
            minute: Duration::from_secs(60),
            busy_retry: Duration::from_secs(5),
            redetect: true,
            fail_watch: false,
        }
    }
}

/// A running monitor. Dropping it stops it.
#[derive(Debug)]
pub struct Monitor {
    cancel: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl Monitor {
    /// Stops the monitor and waits for it to finish.
    pub async fn stop(mut self) {
        self.cancel.cancel();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Engine {
    /// Starts the monitor with the product's timings. Call inside a tokio
    /// runtime; keep the handle for as long as libraries should be watched.
    pub fn start_monitor(&self) -> Monitor {
        self.start_monitor_with(MonitorConfig::default())
    }

    /// Starts the monitor with `config`.
    pub fn start_monitor_with(&self, config: MonitorConfig) -> Monitor {
        let cancel = CancellationToken::new();
        let engine = self.clone();
        let token = cancel.clone();
        // A restart re-enters with the config and lines the libraries up
        // with the database again.
        let task = spawn_supervised(TaskName::Monitor, self.events.clone(), move || {
            supervise(
                engine.clone(),
                config.clone(),
                engine.subscribe(),
                token.clone(),
            )
        });
        Monitor {
            cancel,
            task: Some(task),
        }
    }
}

/// What a library worker was started with; a change restarts it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct WorkerKey {
    path: PathBuf,
    kind: LibraryKind,
    mode: ChangeMode,
}

impl WorkerKey {
    fn of(lib: &Library) -> WorkerKey {
        WorkerKey {
            path: lib.path.clone(),
            kind: lib.kind,
            mode: lib.change_mode,
        }
    }
}

struct Worker {
    key: WorkerKey,
    cancel: CancellationToken,
}

async fn supervise(
    engine: Engine,
    config: MonitorConfig,
    mut events: broadcast::Receiver<EngineEvent>,
    cancel: CancellationToken,
) {
    // Stops this run's library workers however it ends, also when it
    // panics, so a restart does not start them twice.
    let run = cancel.child_token();
    let _stop_workers = run.clone().drop_guard();
    let mut workers: HashMap<LibraryId, Worker> = HashMap::new();
    let start = |workers: &mut HashMap<LibraryId, Worker>, lib: Library| {
        if let Some(old) = workers.remove(&lib.id) {
            old.cancel.cancel();
        }
        let token = run.child_token();
        workers.insert(
            lib.id,
            Worker {
                key: WorkerKey::of(&lib),
                cancel: token.clone(),
            },
        );
        let (engine, config) = (engine.clone(), config.clone());
        spawn_supervised(TaskName::Monitor, engine.events.clone(), move || {
            run_library(engine.clone(), lib.clone(), config.clone(), token.clone())
        });
    };

    let mut libs = engine.libraries().await.unwrap_or_default();
    if config.redetect {
        for lib in &mut libs {
            let path = lib.path.clone();
            let kind = tokio::task::spawn_blocking(move || crate::volume::detect(&path))
                .await
                .unwrap_or(lib.kind);
            if kind != lib.kind && engine.db.set_library_kind(lib.id, kind).await.is_ok() {
                tracing::info!(library = %lib.display_name, kind = kind.as_str(), "library volume kind changed");
                lib.kind = kind;
            }
        }
    }
    for lib in libs {
        start(&mut workers, lib);
    }

    loop {
        crate::fsx::faults::checkpoint_at(TaskName::Monitor, "supervise");
        let event = tokio::select! {
            () = cancel.cancelled() => break,
            e = events.recv() => e,
        };
        match event {
            Ok(EngineEvent::LibraryAdded(lib)) => start(&mut workers, lib),
            Ok(EngineEvent::LibraryRemoved(id)) => {
                if let Some(w) = workers.remove(&id) {
                    w.cancel.cancel();
                }
            }
            Ok(EngineEvent::LibraryChanged(lib)) => {
                if workers.get(&lib.id).map(|w| &w.key) != Some(&WorkerKey::of(&lib)) {
                    start(&mut workers, lib);
                }
            }
            Ok(_) => {}
            Err(broadcast::error::RecvError::Lagged(_)) => {
                // Missed events: line the workers up with the database.
                let libs = engine.libraries().await.unwrap_or_default();
                workers.retain(|id, w| {
                    let keep = libs.iter().any(|l| l.id == *id);
                    if !keep {
                        w.cancel.cancel();
                    }
                    keep
                });
                for lib in libs {
                    if workers.get(&lib.id).map(|w| &w.key) != Some(&WorkerKey::of(&lib)) {
                        start(&mut workers, lib);
                    }
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
    for w in workers.into_values() {
        w.cancel.cancel();
    }
}

/// What the watcher thread tells the library worker.
enum WatchMsg {
    Paths(Vec<PathBuf>),
    /// Events were lost or the watcher failed: rescan everything.
    Lost(String),
}

fn start_watcher(
    root: &Path,
    tx: mpsc::UnboundedSender<WatchMsg>,
) -> notify::Result<notify::RecommendedWatcher> {
    use notify::{EventKind, RecursiveMode, Watcher};
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let msg = match res {
            Ok(ev) if ev.need_rescan() => WatchMsg::Lost("watcher asked for a rescan".into()),
            // Reading files (our own scans, a slicer) changes nothing.
            Ok(ev) if matches!(ev.kind, EventKind::Access(_)) => return,
            Ok(ev) => WatchMsg::Paths(ev.paths),
            Err(e) => WatchMsg::Lost(e.to_string()),
        };
        let _ = tx.send(msg);
    })?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    Ok(watcher)
}

/// `path` relative to `root`, `/`-joined: `Some("")` for the root itself,
/// `None` for paths outside it, non-UTF-8 paths, and Printdex's own
/// sidecars (writing one must not trigger a rescan).
fn relevant(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let mut parts = Vec::new();
    for c in rel.components() {
        parts.push(c.as_os_str().to_str()?);
    }
    if parts.last().is_some_and(|n| is_sidecar_name(n)) {
        return None;
    }
    Some(parts.join("/"))
}

/// Drops paths inside another pending path (`a/b` when `a` is pending).
fn outermost(paths: &BTreeSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in paths {
        // BTreeSet order puts `a` before `a/b`; `a b` sorts between them,
        // so compare against every kept path, not just the last.
        if !out
            .iter()
            .any(|o| p.starts_with(o.as_str()) && p[o.len()..].starts_with('/'))
        {
            out.push(p.clone());
        }
    }
    out
}

/// Changes waiting for the next rescan.
#[derive(Default)]
struct Pending {
    paths: BTreeSet<String>,
    full: bool,
    first: Option<Instant>,
    due: Option<Instant>,
}

impl Pending {
    fn add(&mut self, paths: impl IntoIterator<Item = String>, config: &MonitorConfig) {
        for p in paths {
            if p.is_empty() {
                self.full = true;
            } else {
                self.paths.insert(p);
            }
        }
        if self.paths.len() > config.max_paths {
            self.full = true;
        }
        self.debounce(config);
    }

    fn debounce(&mut self, config: &MonitorConfig) {
        let now = Instant::now();
        let first = *self.first.get_or_insert(now);
        self.due = Some((now + config.debounce).min(first + config.debounce_cap));
    }

    fn now_full(&mut self) {
        self.full = true;
        self.due = Some(Instant::now());
        self.first.get_or_insert(Instant::now());
    }

    fn clear(&mut self) {
        *self = Pending::default();
    }
}

async fn sleep_until(due: Option<Instant>) {
    match due {
        Some(t) => tokio::time::sleep_until(t).await,
        None => std::future::pending().await,
    }
}

async fn tick(timer: &mut Option<tokio::time::Interval>) {
    match timer {
        Some(t) => {
            t.tick().await;
        }
        None => std::future::pending().await,
    }
}

async fn run_library(
    engine: Engine,
    lib: Library,
    config: MonitorConfig,
    cancel: CancellationToken,
) {
    let id = lib.id;
    let mut mode = lib.change_mode.resolve(lib.kind);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut _watcher = None;
    if mode == ChangeMode::Watch {
        let root = lib.path.clone();
        let fail = config.fail_watch;
        let started = tokio::task::spawn_blocking(move || {
            if fail {
                return Err(notify::Error::generic("watching disabled (test)"));
            }
            start_watcher(&root, tx)
        })
        .await;
        match started {
            Ok(Ok(w)) => _watcher = Some(w),
            Ok(Err(e)) => {
                tracing::warn!(library = %lib.display_name, error = %e,
                    "cannot watch library; checking it every {} minutes instead", ChangeMode::SHARE_MINUTES);
                mode = ChangeMode::Every(ChangeMode::SHARE_MINUTES);
            }
            Err(e) => {
                tracing::warn!(library = %lib.display_name, error = %e, "watcher setup failed");
                mode = ChangeMode::Every(ChangeMode::SHARE_MINUTES);
            }
        }
    }
    let mut timer = match mode {
        ChangeMode::Every(m) => {
            let period = config.minute * u32::from(m.max(1));
            let mut t = tokio::time::interval_at(Instant::now() + period, period);
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            Some(t)
        }
        _ => None,
    };
    let mut probe = tokio::time::interval(config.probe_every);
    probe.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut online: Option<bool> = None;
    let mut pending = Pending::default();

    loop {
        crate::fsx::faults::checkpoint_at(TaskName::Monitor, "library");
        tokio::select! {
            () = cancel.cancelled() => break,
            _ = probe.tick() => {
                let now_online =
                    folder_status(engine.fs.clone(), lib.path.clone()).await == LibraryStatus::Online;
                if online != Some(now_online) {
                    let event = if now_online {
                        EngineEvent::LibraryOnline(id)
                    } else {
                        EngineEvent::LibraryOffline(id)
                    };
                    let _ = engine.events.send(event);
                    if online == Some(false) && now_online {
                        tracing::info!(library = %lib.display_name, "library is back online; rescanning");
                        pending.now_full();
                    }
                    online = Some(now_online);
                }
            }
            () = tick(&mut timer) => {
                if online != Some(false) {
                    pending.now_full();
                }
            }
            msg = rx.recv(), if _watcher.is_some() => match msg {
                Some(WatchMsg::Paths(paths)) => {
                    let rels: Vec<String> =
                        paths.iter().filter_map(|p| relevant(&lib.path, p)).collect();
                    if !rels.is_empty() {
                        pending.add(rels, &config);
                    }
                }
                Some(WatchMsg::Lost(why)) => {
                    tracing::warn!(library = %lib.display_name, %why, "watcher lost events; rescanning the library");
                    pending.full = true;
                    pending.debounce(&config);
                }
                None => _watcher = None,
            },
            () = sleep_until(pending.due) => {
                if online == Some(false) {
                    // The online check rescans when it comes back.
                    pending.clear();
                    continue;
                }
                let result = if pending.full {
                    engine.rescan(id, cancel.child_token()).await
                } else {
                    engine
                        .rescan_paths(id, outermost(&pending.paths), cancel.child_token())
                        .await
                };
                match result {
                    Err(CoreError::AlreadyScanning(_)) => {
                        pending.due = Some(Instant::now() + config.busy_retry);
                    }
                    Err(CoreError::LibraryOffline(_)) => {
                        pending.clear();
                        if online != Some(false) {
                            online = Some(false);
                            let _ = engine.events.send(EngineEvent::LibraryOffline(id));
                        }
                    }
                    Err(CoreError::Cancelled) => break,
                    Err(e) => {
                        tracing::warn!(library = %lib.display_name, error = %e, "monitor rescan failed");
                        pending.clear();
                    }
                    Ok(_) => pending.clear(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevant_paths() {
        let root = Path::new("/lib");
        assert_eq!(
            relevant(root, Path::new("/lib/a/b.stl")).as_deref(),
            Some("a/b.stl")
        );
        assert_eq!(relevant(root, Path::new("/lib")).as_deref(), Some(""));
        assert_eq!(relevant(root, Path::new("/elsewhere/x")), None);
        assert_eq!(relevant(root, Path::new("/lib/a/.printdex.toml")), None);
        assert_eq!(relevant(root, Path::new("/lib/.b.stl.printdex.toml")), None);
    }

    #[test]
    fn outermost_drops_nested_paths() {
        let set: BTreeSet<String> = ["a", "a b", "a/b", "a/b/c.stl", "ab", "c/d"]
            .into_iter()
            .map(String::from)
            .collect();
        assert_eq!(outermost(&set), ["a", "a b", "ab", "c/d"]);
    }

    #[test]
    fn many_paths_make_a_full_rescan() {
        let config = MonitorConfig {
            max_paths: 2,
            ..MonitorConfig::default()
        };
        let mut p = Pending::default();
        p.add(["a".to_string(), "b".to_string()], &config);
        assert!(!p.full);
        p.add(["c".to_string()], &config);
        assert!(p.full);
        let mut p = Pending::default();
        p.add([String::new()], &config);
        assert!(p.full, "a change at the root rescans everything");
    }
}
