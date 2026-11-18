//! "Open with": the app the user picked per printable format (a slicer),
//! kept in `apps.toml` next to `settings.toml`.

use std::path::{Path, PathBuf};
use std::process::Command;

use printdex_core::PrintableFormat;

/// File name inside the config folder.
pub const FILE_NAME: &str = "apps.toml";

/// Formats that can have their own app, in Settings order.
pub const FORMATS: [PrintableFormat; 3] = [
    PrintableFormat::Stl,
    PrintableFormat::ThreeMf,
    PrintableFormat::Obj,
];

/// The chosen app per format; `None` uses the system's default app.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Apps {
    pub stl: Option<PathBuf>,
    pub three_mf: Option<PathBuf>,
    pub obj: Option<PathBuf>,
}

fn key(f: PrintableFormat) -> &'static str {
    match f {
        PrintableFormat::Stl => "stl",
        PrintableFormat::ThreeMf => "3mf",
        PrintableFormat::Obj => "obj",
    }
}

impl Apps {
    pub fn get(&self, f: PrintableFormat) -> Option<&Path> {
        match f {
            PrintableFormat::Stl => self.stl.as_deref(),
            PrintableFormat::ThreeMf => self.three_mf.as_deref(),
            PrintableFormat::Obj => self.obj.as_deref(),
        }
    }

    pub fn set(&mut self, f: PrintableFormat, app: Option<PathBuf>) {
        let slot = match f {
            PrintableFormat::Stl => &mut self.stl,
            PrintableFormat::ThreeMf => &mut self.three_mf,
            PrintableFormat::Obj => &mut self.obj,
        };
        *slot = app;
    }

    pub fn parse(text: &str) -> Apps {
        let mut apps = Apps::default();
        let Ok(table) = text.parse::<toml::Table>() else {
            return apps;
        };
        let Some(open_with) = table.get("open-with").and_then(toml::Value::as_table) else {
            return apps;
        };
        for f in FORMATS {
            if let Some(p) = open_with.get(key(f)).and_then(toml::Value::as_str)
                && !p.is_empty()
            {
                apps.set(f, Some(PathBuf::from(p)));
            }
        }
        apps
    }

    pub fn to_toml(&self) -> String {
        let mut open_with = toml::Table::new();
        for f in FORMATS {
            if let Some(p) = self.get(f).and_then(Path::to_str) {
                open_with.insert(key(f).into(), p.into());
            }
        }
        let mut t = toml::Table::new();
        t.insert("open-with".into(), open_with.into());
        t.to_string()
    }

    /// Loads `dir/apps.toml`; empty when missing or unreadable.
    pub fn load(dir: &Path) -> Apps {
        std::fs::read_to_string(dir.join(FILE_NAME))
            .map(|t| Apps::parse(&t))
            .unwrap_or_default()
    }

    /// Writes `dir/apps.toml` atomically.
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{FILE_NAME}.tmp"));
        std::fs::write(&tmp, self.to_toml())?;
        std::fs::rename(&tmp, dir.join(FILE_NAME))
    }
}

/// The name to show for an app ("PrusaSlicer" for `/Applications/PrusaSlicer.app`
/// or `C:\…\prusa-slicer.exe`).
pub fn app_name(app: &Path) -> String {
    app.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| app.display().to_string())
}

/// The program and arguments that open `files` in `app`: the app itself
/// with the files as arguments, or `open -a` for a macOS app bundle. Never
/// a shell, so no file name is ever interpreted.
pub fn open_command(app: &Path, files: &[PathBuf]) -> Command {
    let bundle = app
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("app"));
    let mut cmd = if bundle {
        let mut c = Command::new("open");
        c.arg("-a").arg(app);
        c
    } else {
        Command::new(app)
    };
    cmd.args(files);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_tolerates_junk() {
        let mut a = Apps::default();
        a.set(
            PrintableFormat::Stl,
            Some(PathBuf::from("/usr/bin/prusa-slicer")),
        );
        a.set(
            PrintableFormat::ThreeMf,
            Some(PathBuf::from("/opt/Orca Slicer/orca")),
        );
        assert_eq!(Apps::parse(&a.to_toml()), a);
        assert_eq!(Apps::parse("open-with = 3"), Apps::default());
        assert_eq!(Apps::parse("not toml ["), Apps::default());
        let dir = tempfile::tempdir().unwrap();
        a.save(dir.path()).unwrap();
        assert_eq!(Apps::load(dir.path()), a);
    }

    #[test]
    fn open_with_uses_chosen_app() {
        let files = vec![
            PathBuf::from("/lib/Hook/a b.stl"),
            PathBuf::from("/lib/Hook/c.stl"),
        ];
        let cmd = open_command(Path::new("/usr/bin/prusa-slicer"), &files);
        assert_eq!(cmd.get_program(), "/usr/bin/prusa-slicer");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, ["/lib/Hook/a b.stl", "/lib/Hook/c.stl"]);

        let cmd = open_command(Path::new("/Applications/PrusaSlicer.app"), &files[..1]);
        assert_eq!(cmd.get_program(), "open");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(
            args,
            ["-a", "/Applications/PrusaSlicer.app", "/lib/Hook/a b.stl"]
        );
        assert_eq!(
            app_name(Path::new("/Applications/PrusaSlicer.app")),
            "PrusaSlicer"
        );
        assert_eq!(
            app_name(Path::new("C:/Tools/prusa-slicer.exe")),
            "prusa-slicer"
        );
    }
}
