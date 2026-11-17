use clap::Parser;

/// Printdex: organize your 3D-print model library.
#[derive(Debug, Parser)]
#[command(name = "printdex", version = printdex_core::VERSION, about)]
struct Cli {}

fn main() {
    let _cli = Cli::parse();
}
