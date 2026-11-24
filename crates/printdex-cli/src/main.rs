//! `printdex`: command line front end for the Printdex engine.
#![cfg_attr(
    not(test),
    warn(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap
    )
)]

mod bench;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use printdex_core::{CoreError, Engine, EngineEvent, ScanPhase};
use tokio_util::sync::CancellationToken;

/// Printdex: organize your 3D-print model library.
#[derive(Debug, Parser)]
#[command(name = "printdex", version = printdex_core::VERSION, about)]
struct Cli {
    /// Folder holding the Printdex database (default: the per-user data folder).
    #[arg(long, global = true, value_name = "PATH")]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Add a library folder if needed, then scan it.
    Scan {
        path: PathBuf,
        /// Display name when the library is added.
        #[arg(long)]
        name: Option<String>,
    },
    /// Search models by name and path.
    Search {
        #[arg(required = true)]
        query: Vec<String>,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Print row counts.
    Stats,
    /// Synthetic libraries and benchmarks.
    #[command(subcommand)]
    Bench(BenchCommand),
}

#[derive(Debug, Subcommand)]
enum BenchCommand {
    /// Generate a synthetic model library.
    Gen {
        dir: PathBuf,
        #[arg(long)]
        parts: u64,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Time a scan, an unchanged rescan and searches on a library
    /// (uses a fresh temporary database).
    Run {
        dir: PathBuf,
        #[arg(long, default_value_t = 200)]
        queries: u32,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Milliseconds added to every filesystem call (a slow share).
        #[arg(long, default_value_t = 0)]
        chaos_latency_ms: u64,
        /// Seconds to measure idle CPU with the monitor running (0 skips).
        #[arg(long, default_value_t = 0)]
        idle_secs: u64,
        /// Also write the report as JSON to this file.
        #[arg(long, value_name = "FILE")]
        report: Option<PathBuf>,
    },
    /// Compare a JSON report with budgets and the recorded baseline.
    Check {
        report: PathBuf,
        #[arg(long, default_value = "bench/baseline.json")]
        baseline: PathBuf,
    },
}

/// Exit code when a bench check fails.
const EXIT_BENCH_CHECK: u8 = 3;

/// Exit code when an unchanged rescan read file contents.
const EXIT_BENCH_REFINGERPRINTED: u8 = 2;

/// Exit code for a scan stopped with Ctrl-C (128 + SIGINT).
const EXIT_CANCELLED: u8 = 130;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let _log_guard = init_diagnostics(cli.data_dir.as_deref());
    match run(cli).await {
        Ok(code) => code,
        Err(e) => {
            if matches!(e.downcast_ref::<CoreError>(), Some(CoreError::Cancelled)) {
                eprintln!("cancelled");
                return ExitCode::from(EXIT_CANCELLED);
            }
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Logs to stderr and the data folder's log file, and records panics.
fn init_diagnostics(
    data_dir: Option<&std::path::Path>,
) -> Option<tracing_appender::non_blocking::WorkerGuard> {
    let data_dir = data_dir
        .map(std::path::Path::to_path_buf)
        .or_else(Engine::default_data_dir)?;
    let log_dir = printdex_core::diagnostics::log_dir(&data_dir);
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "warn,sqlx=error".to_string());
    let guard = printdex_core::diagnostics::init(&log_dir, &filter);
    printdex_core::diagnostics::install_panic_hook(
        log_dir,
        env!("CARGO_PKG_VERSION"),
        printdex_core::diagnostics::CrashSource::Cli,
    );
    guard
}

async fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    if let Command::Bench(cmd) = cli.command {
        return run_bench(cmd).await;
    }
    let data_dir = match cli.data_dir {
        Some(d) => d,
        None => Engine::default_data_dir()
            .ok_or_else(|| anyhow::anyhow!("no data folder found; pass --data-dir"))?,
    };
    let engine = Engine::open(&data_dir).await?;
    match cli.command {
        Command::Scan { path, name } => scan(&engine, &path, name).await?,
        Command::Search { query, limit } => {
            for hit in engine.search(&query.join(" "), limit).await? {
                let missing = if hit.missing { "\t(missing)" } else { "" };
                println!(
                    "{}\t{}\t{} parts{missing}",
                    hit.name, hit.rel_path, hit.part_count
                );
            }
        }
        Command::Stats => {
            let s = engine.stats().await?;
            println!("libraries: {}", s.libraries);
            println!("models: {}", s.models);
            println!("parts: {}", s.parts);
            println!("assets: {}", s.assets);
            println!("missing_models: {}", s.missing_models);
        }
        Command::Bench(_) => unreachable!("handled above"),
    }
    Ok(ExitCode::SUCCESS)
}

async fn run_bench(cmd: BenchCommand) -> anyhow::Result<ExitCode> {
    match cmd {
        BenchCommand::Gen { dir, parts, seed } => {
            let r = bench::r#gen::generate(&dir, &bench::r#gen::GenSpec { parts, seed })?;
            println!("models: {}", r.models);
            println!("parts: {}", r.parts);
            println!("assets: {}", r.assets);
            println!("bytes: {}", r.bytes);
            println!("words: {}", r.words.len());
            Ok(ExitCode::SUCCESS)
        }
        BenchCommand::Run {
            dir,
            queries,
            seed,
            chaos_latency_ms,
            idle_secs,
            report,
        } => {
            let opts = bench::run::BenchOptions {
                queries,
                seed,
                chaos_latency: std::time::Duration::from_millis(chaos_latency_ms),
                idle: std::time::Duration::from_secs(idle_secs),
            };
            let r = bench::run::run(&dir, &opts).await?;
            println!("models: {}", r.models);
            println!("initial_scan_ms: {}", r.initial_scan_ms);
            println!("rescan_ms: {}", r.rescan_ms);
            println!("rescan_fingerprinted: {}", r.rescan_fingerprinted);
            println!("subtree_rescan_ms: {}", r.subtree_rescan_ms);
            println!("start_to_usable_ms: {}", r.start_to_usable_ms);
            println!("search_p50_us: {}", r.search_p50_us);
            println!("search_p95_us: {}", r.search_p95_us);
            println!("search_max_us: {}", r.search_max_us);
            println!("queries: {}", r.queries);
            if idle_secs > 0 {
                println!("idle_cpu_pct: {:.2}", r.idle_cpu_pct);
            }
            println!("rss_mb: {}", r.rss_mb);
            if let Some(path) = report {
                std::fs::write(&path, serde_json::to_string_pretty(&r)?)?;
            }
            if r.rescan_fingerprinted != 0 {
                eprintln!(
                    "error: unchanged rescan read {} files",
                    r.rescan_fingerprinted
                );
                return Ok(ExitCode::from(EXIT_BENCH_REFINGERPRINTED));
            }
            Ok(ExitCode::SUCCESS)
        }
        BenchCommand::Check { report, baseline } => {
            let r: bench::run::BenchReport =
                serde_json::from_str(&std::fs::read_to_string(&report)?)?;
            let b: bench::check::Baseline =
                serde_json::from_str(&std::fs::read_to_string(&baseline)?)?;
            let findings = bench::check::compare(&r, &b);
            if b.baseline.is_none() {
                println!("no recorded baseline yet: checking budgets only");
            }
            for f in &findings {
                let tag = if f.fails { "FAIL" } else { "warn" };
                println!("{tag} {}: {}", f.metric, f.message);
            }
            if findings.iter().any(|f| f.fails) {
                return Ok(ExitCode::from(EXIT_BENCH_CHECK));
            }
            println!("ok");
            Ok(ExitCode::SUCCESS)
        }
    }
}

async fn scan(engine: &Engine, path: &Path, name: Option<String>) -> anyhow::Result<()> {
    let lib = match engine.library_by_path(path).await? {
        Some(lib) => lib,
        None => engine.add_library(path, name).await?,
    };

    let cancel = CancellationToken::new();
    let ctrl_c = tokio::spawn({
        let cancel = cancel.clone();
        async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                cancel.cancel();
            }
        }
    });
    let progress = tokio::spawn(print_phases(engine.subscribe(), lib.id));

    let result = engine.rescan(lib.id, cancel).await;
    ctrl_c.abort();
    progress.abort();
    let s = result?;
    println!(
        "models: +{} missing {} returned {} moved {} · files {} · fingerprinted {} · skipped {} · {} ms",
        s.models_added,
        s.models_missing,
        s.models_returned,
        s.models_moved,
        s.files_seen,
        s.fingerprinted,
        s.skipped,
        s.duration_ms
    );
    Ok(())
}

/// Prints one stderr line whenever the scan of `library` enters a new phase.
async fn print_phases(
    mut rx: tokio::sync::broadcast::Receiver<EngineEvent>,
    library: printdex_core::LibraryId,
) {
    let mut last: Option<ScanPhase> = None;
    loop {
        match rx.recv().await {
            Ok(EngineEvent::ScanProgress {
                library: id,
                phase,
                total,
                ..
            }) if id == library && last.as_ref() != Some(&phase) => {
                let label = match phase {
                    ScanPhase::Walking => "walking library".to_string(),
                    ScanPhase::Fingerprinting => format!("fingerprinting {total} files"),
                    ScanPhase::Writing => format!("writing {total} models"),
                };
                eprintln!("{label}…");
                last = Some(phase);
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
        }
    }
}
