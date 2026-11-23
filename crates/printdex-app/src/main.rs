//! Printdex desktop app.

mod app;
mod apps;
mod backup_ui;
mod files;
mod filters;
mod format;
mod grid;
mod i18n;
mod icons;
mod list;
mod log_form;
mod motion;
mod nav;
mod org;
mod prints;
mod report;
mod restore_ui;
mod search;
mod settings;
mod sync_ui;
mod theme;
mod thumbs;
mod updates;
mod view;
mod viewer;

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
    /// Folder for caches such as thumbnails (defaults to the OS cache
    /// folder, or `cache` inside --data-dir when that is given).
    #[arg(long)]
    cache_dir: Option<PathBuf>,
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
    // First thing: Velopack's install/update hooks may run and exit here.
    velopack::VelopackApp::build().run();
    let cli = Cli::parse();
    let cache_dir = match (&cli.cache_dir, &cli.data_dir) {
        (Some(c), _) => c.clone(),
        (None, Some(d)) => d.join("cache"),
        (None, None) => printdex_core::Engine::default_cache_dir()
            .context("no cache folder for this user; pass --cache-dir")?,
    };
    let data_dir = match cli.data_dir {
        Some(d) => d,
        None => printdex_core::Engine::default_data_dir()
            .context("no data folder for this user; pass --data-dir")?,
    };
    let log_dir = printdex_core::diagnostics::log_dir(&data_dir);
    let filter = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| "info,wgpu=warn,naga=warn,iced_wgpu=warn".to_string());
    let _log_guard = printdex_core::diagnostics::init(&log_dir, &filter);
    // Read before the hook is installed, so this run's own crash isn't seen.
    let crash = printdex_core::diagnostics::unseen_crash(&log_dir);
    printdex_core::diagnostics::install_panic_hook(log_dir, env!("CARGO_PKG_VERSION"));
    let config_dir = cli.config_dir.or_else(settings::Settings::default_dir);
    let (settings, settings_status) = config_dir
        .as_deref()
        .map(settings::Settings::load_with_status)
        .unwrap_or_default();
    // The window's transparency is fixed at creation: glass only where the
    // OS blurs and translucency is on.
    let glass_window =
        settings.translucency == settings::Translucency::Auto && theme::blur_supported();
    let options = Options {
        data_dir,
        cache_dir,
        config_dir,
        settings,
        settings_status,
        glass_window,
        crash,
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
                min_size: Some(app::MIN_WINDOW),
                transparent: glass_window,
                blur: glass_window,
                icon: window_icon(),
                ..iced::window::Settings::default()
            })
            .default_font(theme::FONT_BODY);
    for font in theme::FONT_FILES {
        application = application.font(font);
    }
    application.run()?;
    Ok(())
}

/// The app icon for the window and task bar (macOS uses the bundle's).
fn window_icon() -> Option<iced::window::Icon> {
    let png = include_bytes!("../../../packaging/icons/printdex-256.png");
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    iced::window::icon::from_rgba(rgba.into_raw(), w, h).ok()
}

#[cfg(test)]
mod icon_tests {
    #[test]
    fn window_icon_decodes() {
        assert!(super::window_icon().is_some());
    }
}
