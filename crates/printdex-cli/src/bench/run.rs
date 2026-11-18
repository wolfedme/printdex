//! Timing harness: initial scan, unchanged rescan, subtree rescan, start
//! to usable, search latency, idle CPU and memory.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use printdex_core::Engine;
use printdex_core::fs::{ChaosFs, Fs, RealFs};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::r#gen::VOCABULARY;

/// Benchmark results. Times in milliseconds, except search (microseconds).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BenchReport {
    pub models: u64,
    pub initial_scan_ms: u64,
    pub rescan_ms: u64,
    /// Files whose contents the unchanged rescan read (must be 0).
    pub rescan_fingerprinted: u64,
    /// Rescan of one model folder, as after a watcher event.
    pub subtree_rescan_ms: u64,
    /// Reopening the engine on the filled database plus the first page of
    /// results: what the app waits for before the grid shows.
    pub start_to_usable_ms: u64,
    pub search_p50_us: u64,
    pub search_p95_us: u64,
    pub search_max_us: u64,
    pub queries: u32,
    /// CPU used while idle with the monitor watching 3 libraries, in
    /// percent of one core (0 when the idle phase was skipped).
    pub idle_cpu_pct: f64,
    /// Resident memory of the bench process at the end, in MB.
    pub rss_mb: u64,
}

/// How to run the bench.
#[derive(Clone, Debug)]
pub struct BenchOptions {
    pub queries: u32,
    pub seed: u64,
    /// Delay added to every filesystem call, like a slow network share.
    pub chaos_latency: Duration,
    /// How long to measure idle CPU (0 skips it).
    pub idle: Duration,
}

/// Results per search, like a UI result page.
const SEARCH_LIMIT: u32 = 100;

/// Scans `library` twice into a fresh temporary database, rescans one
/// model folder, reopens the engine, runs `queries` prefix searches (2–5
/// characters of vocabulary words), then idles with the monitor running.
pub async fn run(library: &Path, opts: &BenchOptions) -> anyhow::Result<BenchReport> {
    let data = tempfile::tempdir()?;
    let fs: Arc<dyn Fs> = if opts.chaos_latency.is_zero() {
        Arc::new(RealFs)
    } else {
        Arc::new(ChaosFs::new().with_latency(opts.chaos_latency))
    };
    let cache = data.path().join("cache");
    let engine = Engine::open_with_fs(data.path(), &cache, fs.clone()).await?;
    let lib = engine.add_library(library, None).await?;

    let t = Instant::now();
    engine.rescan(lib.id, CancellationToken::new()).await?;
    let initial_scan_ms = ms(t);

    let t = Instant::now();
    let rescan = engine.rescan(lib.id, CancellationToken::new()).await?;
    let rescan_ms = ms(t);

    let models = engine.stats().await?.models;
    let folder = engine
        .search("", 1)
        .await?
        .into_iter()
        .next()
        .map(|h| h.rel_path);
    let subtree_rescan_ms = match folder {
        Some(rel) => {
            let t = Instant::now();
            engine
                .rescan_paths(lib.id, vec![rel], CancellationToken::new())
                .await?;
            ms(t)
        }
        None => 0,
    };
    drop(engine);

    let t = Instant::now();
    let engine = Engine::open_with_fs(data.path(), &cache, fs).await?;
    engine.search("", SEARCH_LIMIT).await?;
    let start_to_usable_ms = ms(t);

    let vocab: Vec<&str> = VOCABULARY.split_whitespace().collect();
    let mut rng = fastrand::Rng::with_seed(opts.seed);
    let mut times = Vec::with_capacity(opts.queries as usize);
    for _ in 0..opts.queries {
        let word = vocab[rng.usize(..vocab.len())];
        let len = rng.usize(2..=5);
        let prefix: String = word.chars().take(len).collect();
        let t = Instant::now();
        engine.search(&prefix, SEARCH_LIMIT).await?;
        times.push(u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX));
    }
    times.sort_unstable();
    let pct = |p: f64| -> u64 {
        if times.is_empty() {
            return 0;
        }
        let i = ((times.len() as f64 * p).ceil() as usize).clamp(1, times.len()) - 1;
        times[i]
    };

    let idle_cpu_pct = if opts.idle.is_zero() {
        0.0
    } else {
        idle_cpu(&engine, opts.idle).await?
    };

    Ok(BenchReport {
        models,
        initial_scan_ms,
        rescan_ms,
        rescan_fingerprinted: rescan.fingerprinted,
        subtree_rescan_ms,
        start_to_usable_ms,
        search_p50_us: pct(0.50),
        search_p95_us: pct(0.95),
        search_max_us: times.last().copied().unwrap_or(0),
        queries: opts.queries,
        idle_cpu_pct,
        rss_mb: process_stats().map(|(_, rss)| rss / 1_048_576).unwrap_or(0),
    })
}

/// Adds two small extra libraries, starts the monitor and measures the
/// CPU the process uses over `idle`, after a settling second.
async fn idle_cpu(engine: &Engine, idle: Duration) -> anyhow::Result<f64> {
    let extra = tempfile::tempdir()?;
    for name in ["a", "b"] {
        let dir = extra.path().join(name);
        std::fs::create_dir_all(dir.join("Hook"))?;
        std::fs::write(dir.join("Hook/hook.stl"), b"solid hook\nendsolid hook\n")?;
        let lib = engine.add_library(&dir, None).await?;
        engine.rescan(lib.id, CancellationToken::new()).await?;
    }
    let monitor = engine.start_monitor();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let before = process_stats().map(|(cpu, _)| cpu);
    let t = Instant::now();
    tokio::time::sleep(idle).await;
    let after = process_stats().map(|(cpu, _)| cpu);
    let wall = t.elapsed().as_secs_f64() * 1000.0;
    drop(monitor);
    Ok(match (before, after) {
        (Some(b), Some(a)) if wall > 0.0 => (a.saturating_sub(b) as f64 / wall) * 100.0,
        _ => 0.0,
    })
}

/// This process's accumulated CPU time (ms) and resident memory (bytes).
fn process_stats() -> Option<(u64, u64)> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let pid = sysinfo::get_current_pid().ok()?;
    let mut sys = System::new().ok()?;
    sys.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );
    let p = sys.process(pid)?;
    Some((p.accumulated_cpu_time(), p.memory()))
}

fn ms(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
}
