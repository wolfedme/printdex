//! `printdex`: command line front end for the Printdex engine.

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
    },
}

/// Exit code when an unchanged rescan read file contents.
const EXIT_BENCH_REFINGERPRINTED: u8 = 2;

/// Exit code for a scan stopped with Ctrl-C (128 + SIGINT).
const EXIT_CANCELLED: u8 = 130;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,sqlx=error")),
        )
        .init();

    let cli = Cli::parse();
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
        BenchCommand::Run { dir, queries, seed } => {
            let r = bench::run::run(&dir, queries, seed).await?;
            println!("initial_scan_ms: {}", r.initial_scan_ms);
            println!("rescan_ms: {}", r.rescan_ms);
            println!("rescan_fingerprinted: {}", r.rescan_fingerprinted);
            println!("search_p50_us: {}", r.search_p50_us);
            println!("search_p95_us: {}", r.search_p95_us);
            println!("search_max_us: {}", r.search_max_us);
            println!("queries: {}", r.queries);
            if r.rescan_fingerprinted != 0 {
                eprintln!(
                    "error: unchanged rescan read {} files",
                    r.rescan_fingerprinted
                );
                return Ok(ExitCode::from(EXIT_BENCH_REFINGERPRINTED));
            }
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
        "models: +{} missing {} returned {} · files {} · fingerprinted {} · skipped {} · {} ms",
        s.models_added,
        s.models_missing,
        s.models_returned,
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
