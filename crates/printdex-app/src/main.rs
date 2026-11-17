//! Printdex desktop app.

mod app;
mod format;
mod grid;
mod i18n;
mod icons;
mod nav;
mod search;
mod theme;
mod view;

use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;

use crate::app::{App, Options};

#[derive(Parser)]
#[command(name = "printdex-app", version, about = "Printdex desktop app")]
struct Cli {
    /// Folder for Printdex's database (defaults to the OS data folder).
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Force light or dark instead of following the OS (until Settings exists).
    #[arg(long, value_enum)]
    mode: Option<ModeArg>,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ModeArg {
    Light,
    Dark,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,wgpu=warn,naga=warn,iced_wgpu=warn".into()),
        )
        .init();

    let cli = Cli::parse();
    let data_dir = match cli.data_dir {
        Some(d) => d,
        None => printdex_core::Engine::default_data_dir()
            .context("no data folder for this user; pass --data-dir")?,
    };
    let options = Options {
        data_dir,
        mode: cli.mode.map(|m| match m {
            ModeArg::Light => theme::Mode::Light,
            ModeArg::Dark => theme::Mode::Dark,
        }),
    };

    let mut application =
        iced::application(move || App::boot(options.clone()), App::update, view::view)
            .title(App::title)
            .theme(App::theme)
            .subscription(App::subscription)
            .window_size(app::INITIAL_WINDOW)
            .default_font(theme::FONT_BODY);
    for font in theme::FONT_FILES {
        application = application.font(font);
    }
    application.run()?;
    Ok(())
}
