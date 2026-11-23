//! The user's settings, kept in `settings.toml` in the OS config folder.
//!
//! Loading is tolerant: a missing file, a file that does not parse, or a
//! field with an unknown or wrongly typed value falls back to the default
//! for that field only. Startup never fails because of this file.

use std::path::{Path, PathBuf};

use printdex_core::SortOrder;

use crate::theme::ColorTheme;

/// File name inside the config folder.
pub const FILE_NAME: &str = "settings.toml";

/// Light, dark, or follow the OS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// How the library shows models.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    Grid,
    List,
}

/// Window translucency: glass where the OS can blur, or always solid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Translucency {
    #[default]
    Auto,
    Off,
}

/// Thumbnail cache size limits on offer, in megabytes (1,000,000 bytes).
pub const CACHE_LIMITS_MB: [u32; 5] = [250, 500, 1000, 2000, 5000];
/// The default cache limit, 1 GB.
pub const DEFAULT_CACHE_MB: u32 = 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub color_theme: ColorTheme,
    pub mode: ThemeMode,
    pub view: ViewMode,
    pub sort: SortOrder,
    pub translucency: Translucency,
    pub reduce_motion: bool,
    /// Thumbnail cache size limit, one of [`CACHE_LIMITS_MB`].
    pub cache_mb: u32,
    /// Check for updates automatically: `None` until the user answered.
    pub update_checks: Option<bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            color_theme: ColorTheme::default(),
            mode: ThemeMode::default(),
            view: ViewMode::default(),
            sort: SortOrder::default(),
            translucency: Translucency::default(),
            reduce_motion: false,
            cache_mb: DEFAULT_CACHE_MB,
            update_checks: None,
        }
    }
}

impl Settings {
    /// The cache limit in bytes.
    pub fn cache_bytes(&self) -> u64 {
        u64::from(self.cache_mb) * 1_000_000
    }
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark];

    fn key(self) -> &'static str {
        match self {
            ThemeMode::System => "system",
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        }
    }

    fn from_key(key: &str) -> Option<ThemeMode> {
        ThemeMode::ALL.into_iter().find(|m| m.key() == key)
    }
}

fn view_key(v: ViewMode) -> &'static str {
    match v {
        ViewMode::Grid => "grid",
        ViewMode::List => "list",
    }
}

fn sort_key(s: SortOrder) -> &'static str {
    match s {
        // Collection order applies only inside a collection, never saved.
        SortOrder::Name | SortOrder::Collection => "name",
        SortOrder::RecentlyAdded => "recently-added",
    }
}

fn translucency_key(t: Translucency) -> &'static str {
    match t {
        Translucency::Auto => "auto",
        Translucency::Off => "off",
    }
}

/// Renames a config file that doesn't parse to `<name>.broken-<unix_ms>`
/// beside it and says where it went; `None` when the rename failed (logged).
pub(crate) fn set_aside(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    for n in 0..100 {
        let suffix = if n == 0 {
            format!("{at}")
        } else {
            format!("{at}-{n}")
        };
        let to = path.with_file_name(format!("{name}.broken-{suffix}"));
        match printdex_core::fsx::rename_no_replace(path, &to) {
            Ok(()) => {
                tracing::warn!(from = %path.display(), to = %to.display(), "config file does not parse; set aside");
                return Some(to);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "could not set aside broken config file");
                return None;
            }
        }
    }
    None
}

impl Settings {
    /// The settings file in the OS config folder.
    pub fn default_dir() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "Printdex").map(|d| d.config_dir().to_path_buf())
    }

    /// Reads settings from `text`, keeping defaults for anything missing or
    /// not understood.
    pub fn parse(text: &str) -> Settings {
        let mut s = Settings::default();
        let table = match text.parse::<toml::Table>() {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(error = %e, "settings file does not parse; using defaults");
                return s;
            }
        };
        let str_field = |name: &str| table.get(name).and_then(toml::Value::as_str);
        if let Some(v) = str_field("color-theme").and_then(ColorTheme::from_key) {
            s.color_theme = v;
        }
        if let Some(v) = str_field("mode").and_then(ThemeMode::from_key) {
            s.mode = v;
        }
        if let Some(v) = str_field("view").and_then(|k| {
            [ViewMode::Grid, ViewMode::List]
                .into_iter()
                .find(|v| view_key(*v) == k)
        }) {
            s.view = v;
        }
        if let Some(v) = str_field("sort").and_then(|k| {
            [SortOrder::Name, SortOrder::RecentlyAdded]
                .into_iter()
                .find(|v| sort_key(*v) == k)
        }) {
            s.sort = v;
        }
        if let Some(v) = str_field("translucency").and_then(|k| {
            [Translucency::Auto, Translucency::Off]
                .into_iter()
                .find(|v| translucency_key(*v) == k)
        }) {
            s.translucency = v;
        }
        if let Some(v) = table.get("reduce-motion").and_then(toml::Value::as_bool) {
            s.reduce_motion = v;
        }
        if let Some(v) = table
            .get("cache-limit-mb")
            .and_then(toml::Value::as_integer)
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| CACHE_LIMITS_MB.contains(v))
        {
            s.cache_mb = v;
        }
        s.update_checks = table.get("update-checks").and_then(toml::Value::as_bool);
        s
    }

    /// The file's text.
    pub fn to_toml(self) -> String {
        let mut t = toml::Table::new();
        t.insert("color-theme".into(), self.color_theme.key().into());
        t.insert("mode".into(), self.mode.key().into());
        t.insert("view".into(), view_key(self.view).into());
        t.insert("sort".into(), sort_key(self.sort).into());
        t.insert(
            "translucency".into(),
            translucency_key(self.translucency).into(),
        );
        t.insert("reduce-motion".into(), self.reduce_motion.into());
        t.insert("cache-limit-mb".into(), i64::from(self.cache_mb).into());
        if let Some(v) = self.update_checks {
            t.insert("update-checks".into(), v.into());
        }
        t.to_string()
    }

    /// Loads `dir/settings.toml`; defaults when it is missing or unreadable.
    /// A file that doesn't parse is set aside (see [`Settings::load_with_status`]).
    #[cfg(test)]
    pub fn load(dir: &Path) -> Settings {
        Settings::load_with_status(dir).0
    }

    /// Like [`Settings::load`], plus where a file that didn't parse was set
    /// aside (`settings.toml.broken-<unix_ms>`). A file that can't be read
    /// at all is left where it is.
    pub fn load_with_status(dir: &Path) -> (Settings, Option<PathBuf>) {
        let path = dir.join(FILE_NAME);
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                if text.parse::<toml::Table>().is_err() {
                    let aside = set_aside(&path);
                    return (Settings::default(), aside);
                }
                (Settings::parse(&text), None)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Settings::default(), None),
            Err(e) => {
                tracing::warn!(error = %e, "could not read settings; using defaults");
                (Settings::default(), None)
            }
        }
    }

    /// Writes `dir/settings.toml` atomically: a temporary file in the same
    /// folder, then a rename over the old one.
    pub fn save(self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        printdex_core::fsx::durable_write(
            &dir.join(FILE_NAME),
            self.to_toml().as_bytes(),
            printdex_core::fsx::Durability::UserData,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed() -> Settings {
        Settings {
            color_theme: ColorTheme::SeaGlass,
            mode: ThemeMode::Dark,
            view: ViewMode::List,
            sort: SortOrder::RecentlyAdded,
            translucency: Translucency::Off,
            reduce_motion: true,
            cache_mb: 5000,
            update_checks: Some(true),
        }
    }

    #[test]
    fn defaults_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(dir.path()), Settings::default());
        assert_eq!(Settings::default().color_theme, ColorTheme::GraphiteSignal);
        assert_eq!(Settings::default().mode, ThemeMode::System);
    }

    #[test]
    fn settings_save_uses_unique_temp() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|s| {
            for _ in 0..2 {
                s.spawn(|| {
                    barrier.wait();
                    for _ in 0..50 {
                        changed().save(dir.path()).unwrap();
                    }
                });
            }
        });
        assert_eq!(Settings::load(dir.path()), changed());
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, [FILE_NAME]);
    }

    #[test]
    fn broken_settings_are_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(FILE_NAME), "mode = 3 =").unwrap();
        let (s, aside) = Settings::load_with_status(dir.path());
        assert_eq!(s, Settings::default());
        let aside = aside.expect("set aside");
        assert!(
            aside
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("settings.toml.broken-")
        );
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "mode = 3 =");
        assert!(!dir.path().join(FILE_NAME).exists());
        // Loading again finds nothing to set aside.
        assert_eq!(Settings::load_with_status(dir.path()).1, None);
    }

    #[test]
    fn unreadable_settings_are_not_renamed() {
        let dir = tempfile::tempdir().unwrap();
        // A directory in the file's place: reading fails, parsing never runs.
        std::fs::create_dir(dir.path().join(FILE_NAME)).unwrap();
        let (s, aside) = Settings::load_with_status(dir.path());
        assert_eq!(s, Settings::default());
        assert_eq!(aside, None);
        assert!(dir.path().join(FILE_NAME).is_dir());
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        changed().save(dir.path()).unwrap();
        assert_eq!(Settings::load(dir.path()), changed());
    }

    #[test]
    fn settings_tolerate_bad_values() {
        let text = r#"
            color-theme = "neon"
            mode = 3
            view = "list"
            reduce-motion = "yes"
            sort = "recently-added"
            cache-limit-mb = 777
        "#;
        let s = Settings::parse(text);
        assert_eq!(s.color_theme, ColorTheme::GraphiteSignal);
        assert_eq!(s.mode, ThemeMode::System);
        assert_eq!(s.view, ViewMode::List);
        assert!(!s.reduce_motion);
        assert_eq!(s.sort, SortOrder::RecentlyAdded);
        assert_eq!(s.cache_mb, DEFAULT_CACHE_MB, "only offered limits");
        // Truncated file: defaults.
        assert_eq!(Settings::parse("color-theme = \"sea"), Settings::default());
    }

    #[test]
    fn save_is_atomic() {
        let dir = tempfile::tempdir().unwrap();
        Settings::default().save(dir.path()).unwrap();
        changed().save(dir.path()).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, vec![FILE_NAME.to_owned()]);
        assert_eq!(Settings::load(dir.path()), changed());
    }
}
