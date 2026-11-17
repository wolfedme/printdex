//! Printdex desktop app.

mod app;
mod format;
mod grid;
mod i18n;
mod icons;
mod list;
mod nav;
mod search;
mod settings;
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
    /// Folder for settings.toml (defaults to the OS config folder).
    #[arg(long)]
    config_dir: Option<PathBuf>,
    /// Use light or dark for this run only, ignoring the setting.
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
    let config_dir = cli.config_dir.or_else(settings::Settings::default_dir);
    let settings = config_dir
        .as_deref()
        .map(settings::Settings::load)
        .unwrap_or_default();
    // The window's transparency is fixed at creation: glass only where the
    // OS blurs and translucency is on.
    let glass_window =
        settings.translucency == settings::Translucency::Auto && theme::blur_supported();
    let options = Options {
        data_dir,
        config_dir,
        settings,
        glass_window,
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
            .window(iced::window::Settings {
                size: app::INITIAL_WINDOW,
                transparent: glass_window,
                blur: glass_window,
                ..iced::window::Settings::default()
            })
            .default_font(theme::FONT_BODY);
    for font in theme::FONT_FILES {
        application = application.font(font);
    }
    application.run()?;
    Ok(())
}
