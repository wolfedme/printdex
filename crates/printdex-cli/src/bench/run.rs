//! Timing harness: initial scan, unchanged rescan, and search latency.

use std::path::Path;
use std::time::Instant;

use printdex_core::Engine;
use tokio_util::sync::CancellationToken;

use super::r#gen::VOCABULARY;

/// Benchmark results. Times in milliseconds (scans) and microseconds (search).
#[derive(Clone, Debug, Default)]
pub struct BenchReport {
    pub initial_scan_ms: u64,
    pub rescan_ms: u64,
    pub rescan_fingerprinted: u64,
    pub search_p50_us: u64,
    pub search_p95_us: u64,
    pub search_max_us: u64,
    pub queries: u32,
}

/// Results per search, like a UI result page.
const SEARCH_LIMIT: u32 = 100;

/// Scans `library` twice into a fresh temporary database, then runs
/// `queries` prefix searches (2–5 characters of vocabulary words).
pub async fn run(library: &Path, queries: u32, seed: u64) -> anyhow::Result<BenchReport> {
    let data = tempfile::tempdir()?;
    let engine = Engine::open(data.path()).await?;
    let lib = engine.add_library(library, None).await?;

    let t = Instant::now();
    engine.rescan(lib.id, CancellationToken::new()).await?;
    let initial_scan_ms = ms(t);

    let t = Instant::now();
    let rescan = engine.rescan(lib.id, CancellationToken::new()).await?;
    let rescan_ms = ms(t);

    let vocab: Vec<&str> = VOCABULARY.split_whitespace().collect();
    let mut rng = fastrand::Rng::with_seed(seed);
    let mut times = Vec::with_capacity(queries as usize);
    for _ in 0..queries {
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

    Ok(BenchReport {
        initial_scan_ms,
        rescan_ms,
        rescan_fingerprinted: rescan.fingerprinted,
        search_p50_us: pct(0.50),
        search_p95_us: pct(0.95),
        search_max_us: times.last().copied().unwrap_or(0),
        queries,
    })
}

fn ms(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_millis()).unwrap_or(u64::MAX)
}
