//! Log file, crash records and the diagnostics bundle.
//!
//! [`init`] installs the tracing subscriber (stderr plus a daily rolling log
//! file), [`install_panic_hook`] writes a `crash-<unix_ms>.txt` (the app)
//! or `cli-crash-<unix_ms>.txt` (the command line) for every panic, and [`save_bundle`] zips the newest logs and crash records, with
//! user paths removed by a [`Redactor`], for a bug report.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing_appender::non_blocking::{NonBlocking, WorkerGuard};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

const LOG_PREFIX: &str = "printdex.log";
const CRASH_PREFIX: &str = "crash-";
const CLI_CRASH_PREFIX: &str = "cli-crash-";
const CRASH_SUFFIX: &str = ".txt";
const SEEN_MARKER: &str = ".crash-seen";
const MAX_LOG_FILES: usize = 7;
const BUNDLE_LOGS: usize = 3;
const BUNDLE_CRASHES: usize = 5;

/// The folder holding logs and crash records inside the data directory.
pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

fn make_file_writer(dir: &Path) -> io::Result<(NonBlocking, WorkerGuard)> {
    std::fs::create_dir_all(dir)?;
    let appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(LOG_PREFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(dir)
        .map_err(|e| io::Error::other(e.to_string()))?;
    Ok(tracing_appender::non_blocking(appender))
}

/// Installs the global subscriber: stderr plus a daily rolling file
/// `printdex.log` keeping 7 files. If the folder can't be created or
/// written, only stderr is used and `None` is returned. Keep the guard alive
/// for the life of the process so buffered lines are flushed.
pub fn init(log_dir: &Path, filter: &str) -> Option<WorkerGuard> {
    let filter = || EnvFilter::try_new(filter).unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_filter(filter());
    let (file_layer, guard) = match make_file_writer(log_dir) {
        Ok((writer, guard)) => {
            let layer = tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_filter(filter());
            (Some(layer), Some(guard))
        }
        Err(e) => {
            eprintln!("printdex: log file unavailable ({e}); logging to stderr only");
            (None, None)
        }
    };
    // A second call finds the global subscriber taken; that is not an error.
    let _ = tracing_subscriber::registry()
        .with(stderr)
        .with(file_layer)
        .try_init();
    guard
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Which program a crash record comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashSource {
    /// The desktop app: `crash-<unix_ms>.txt`, shown by the app's crash
    /// notice ([`unseen_crash`]).
    App,
    /// The command line: `cli-crash-<unix_ms>.txt`, only bundled.
    Cli,
}

impl CrashSource {
    fn prefix(self) -> &'static str {
        match self {
            CrashSource::App => CRASH_PREFIX,
            CrashSource::Cli => CLI_CRASH_PREFIX,
        }
    }
}

/// Creates `<prefix><at>.txt` holding `body`, never replacing a file: a
/// second crash in the same millisecond gets `<prefix><at>-1.txt`, then
/// `-2`, and so on.
fn write_crash_file(dir: &Path, prefix: &str, at: i64, body: &str) -> io::Result<PathBuf> {
    let mut n = 0u32;
    loop {
        let name = if n == 0 {
            format!("{prefix}{at}{CRASH_SUFFIX}")
        } else {
            format!("{prefix}{at}-{n}{CRASH_SUFFIX}")
        };
        let path = dir.join(name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(body.as_bytes())?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists && n < 1000 => n += 1,
            Err(e) => return Err(e),
        }
    }
}

/// Writes one crash file. Never panics.
fn write_crash(
    dir: &Path,
    source: CrashSource,
    version: &str,
    message: &str,
    location: &str,
    thread: &str,
) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let at = unix_ms();
    let backtrace = std::backtrace::Backtrace::force_capture();
    let body = format!(
        "Printdex {version} crashed\ntime: {at}\nos: {} {}\nthread: {thread}\nmessage: {}\nlocation: {location}\nbacktrace:\n{backtrace}\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
        message.replace(['\r', '\n'], " "),
    );
    write_crash_file(dir, source.prefix(), at, &body)
}

fn payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic".to_string())
}

fn format_location(l: &std::panic::Location<'_>) -> String {
    format!("{}:{}:{}", l.file(), l.line(), l.column())
}

/// Writes a crash file for `source` into `log_dir` on every panic, then
/// calls the previous hook.
pub fn install_panic_hook(log_dir: PathBuf, version: &'static str, source: CrashSource) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = payload_message(info.payload());
        let location = info
            .location()
            .map_or_else(|| "unknown".to_string(), format_location);
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        let _ = write_crash(&log_dir, source, version, &message, &location, name);
        previous(info);
    }));
}

/// A crash record left by an earlier run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashReport {
    /// The crash file.
    pub path: PathBuf,
    /// When the crash happened, in unix milliseconds.
    pub at_ms: i64,
    /// The panic message.
    pub message: String,
    /// Where it panicked, `file:line:col`.
    pub location: String,
}

/// The `<prefix><ms>[-<n>].txt` files in `dir` with their time, newest
/// first (within a millisecond, the highest `n` first).
fn crash_files(dir: &Path, prefix: &str) -> Vec<(i64, PathBuf)> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<((i64, u32), PathBuf)> = read
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_prefix(prefix)?.strip_suffix(CRASH_SUFFIX)?;
            let (ms, n) = match stem.split_once('-') {
                Some((ms, n)) => (ms.parse().ok()?, n.parse().ok()?),
                None => (stem.parse().ok()?, 0),
            };
            Some(((ms, n), e.path()))
        })
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.into_iter().map(|((ms, _), p)| (ms, p)).collect()
}

/// The newest app crash file (`crash-*`, not the command line's) written
/// after the last [`mark_crashes_seen`].
pub fn unseen_crash(log_dir: &Path) -> Option<CrashReport> {
    let seen = std::fs::metadata(log_dir.join(SEEN_MARKER))
        .and_then(|m| m.modified())
        .ok();
    let (at_ms, path) = crash_files(log_dir, CRASH_PREFIX).into_iter().next()?;
    if let Some(seen) = seen {
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
        if modified <= seen {
            return None;
        }
    }
    let text = std::fs::read_to_string(&path).ok()?;
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .unwrap_or_default()
            .to_string()
    };
    Some(CrashReport {
        path,
        at_ms,
        message: field("message: "),
        location: field("location: "),
    })
}

/// Hides the crashes written so far from [`unseen_crash`].
pub fn mark_crashes_seen(log_dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(log_dir)?;
    std::fs::write(log_dir.join(SEEN_MARKER), b"")
}

/// Replaces library folders with `<library N>` and the home folder with `~`.
#[derive(Debug, Clone)]
pub struct Redactor {
    /// Normalised needle and its replacement, most specific first.
    rules: Vec<(Vec<u8>, String)>,
}

/// Lowercases ASCII, turns `\` into `/` and collapses runs of separators
/// (so `C:\\Users` from a `{:?}` dump matches `C:\Users`). The second vector
/// maps each normalised byte to its offset in `s`, with `s.len()` appended.
fn normalise(s: &str) -> (Vec<u8>, Vec<usize>) {
    let mut out = Vec::with_capacity(s.len());
    let mut map = Vec::with_capacity(s.len() + 1);
    for (i, b) in s.bytes().enumerate() {
        let is_sep = b == b'\\' || b == b'/';
        if is_sep && out.last() == Some(&b'/') {
            continue;
        }
        out.push(if is_sep { b'/' } else { b.to_ascii_lowercase() });
        map.push(i);
    }
    map.push(s.len());
    (out, map)
}

/// Whether the byte after a matched folder ends the path component: a
/// separator or punctuation, not the middle of a longer name. A `.` ends it
/// only when no name character follows (`/home/max.` at a sentence end).
fn ends_component(b: u8, after: Option<u8>) -> bool {
    match b {
        b'.' => after.is_none_or(|a| !a.is_ascii_alphanumeric() && a < 0x80),
        b'_' | b'-' => false,
        _ => b == b'/' || (b.is_ascii() && !b.is_ascii_alphanumeric()),
    }
}

impl Redactor {
    /// Libraries are numbered from 1 in the order given.
    pub fn new(home: Option<&Path>, libraries: &[PathBuf]) -> Redactor {
        let mut rules = Vec::new();
        let mut add = |path: &Path, replacement: String| {
            let (mut needle, _) = normalise(&path.to_string_lossy());
            while needle.last() == Some(&b'/') {
                needle.pop();
            }
            if !needle.is_empty() {
                rules.push((needle, replacement));
            }
        };
        for (i, lib) in libraries.iter().enumerate() {
            add(lib, format!("<library {}>", i + 1));
        }
        if let Some(home) = home {
            add(home, "~".to_string());
        }
        Redactor { rules }
    }

    /// Returns `text` with the known folders replaced. Only whole path
    /// components match, so `/home/max` leaves `/home/maxwell` alone.
    pub fn apply(&self, text: &str) -> String {
        let (norm, map) = normalise(text);
        let mut out = String::with_capacity(text.len());
        let mut i = 0;
        let mut copied = 0;
        while i < norm.len() {
            let hit = self.rules.iter().find(|(needle, _)| {
                norm[i..].starts_with(needle)
                    && norm
                        .get(i + needle.len())
                        .is_none_or(|&b| ends_component(b, norm.get(i + needle.len() + 1).copied()))
            });
            match hit {
                Some((needle, replacement)) => {
                    out.push_str(&text[copied..map[i]]);
                    out.push_str(replacement);
                    i += needle.len();
                    copied = map[i];
                }
                None => i += 1,
            }
        }
        out.push_str(&text[copied..]);
        out
    }
}

/// The version, OS and renderer, as plain text for a bug report.
pub fn system_summary(version: &str, renderer: &str) -> String {
    let os_version = sysinfo::System::long_os_version().unwrap_or_else(|_| "unknown".to_string());
    format!(
        "Printdex {version}\nos: {} {}\nos version: {os_version}\nrenderer: {renderer}\n",
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
}

/// Writes a zip with `summary.txt`, the 3 newest `printdex.log*` files and
/// the 5 newest crash records (`crash-*.txt` and `cli-crash-*.txt`), each
/// passed through `redactor`.
pub fn save_bundle(
    log_dir: &Path,
    dest: &Path,
    summary: &str,
    redactor: &Redactor,
) -> io::Result<()> {
    let mut logs: Vec<PathBuf> = std::fs::read_dir(log_dir)
        .map(|r| r.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    logs.retain(|p| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with(LOG_PREFIX))
    });
    // Dated suffixes sort chronologically, so the largest names are newest.
    logs.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
    logs.truncate(BUNDLE_LOGS);
    let mut crashes = crash_files(log_dir, CRASH_PREFIX);
    crashes.extend(crash_files(log_dir, CLI_CRASH_PREFIX));
    // Stable: within a millisecond each prefix keeps its newest-first order.
    crashes.sort_by_key(|c| std::cmp::Reverse(c.0));
    let crashes: Vec<PathBuf> = crashes
        .into_iter()
        .take(BUNDLE_CRASHES)
        .map(|(_, p)| p)
        .collect();

    let zip_err = |e: zip::result::ZipError| io::Error::other(e.to_string());
    let mut zip = zip::ZipWriter::new(std::fs::File::create(dest)?);
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("summary.txt", options).map_err(zip_err)?;
    zip.write_all(redactor.apply(summary).as_bytes())?;
    for path in logs.iter().chain(&crashes) {
        let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let bytes = std::fs::read(path)?;
        zip.start_file(name, options).map_err(zip_err)?;
        zip.write_all(redactor.apply(&String::from_utf8_lossy(&bytes)).as_bytes())?;
    }
    zip.finish().map_err(zip_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::time::Duration;

    #[test]
    fn file_writer_fails_when_folder_cannot_be_created() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("afile");
        std::fs::write(&file, b"x").unwrap();
        assert!(make_file_writer(&file.join("logs")).is_err());
        let (_writer, _guard) = make_file_writer(&tmp.path().join("logs")).unwrap();
    }

    #[test]
    fn panic_payloads_become_messages() {
        let caught = |f: fn()| std::panic::catch_unwind(f).unwrap_err();
        let silent = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let literal = caught(|| panic!("boom"));
        let formatted = caught(|| panic!("n={}", 7));
        let other = caught(|| std::panic::panic_any(42));
        std::panic::set_hook(silent);
        assert_eq!(payload_message(&*literal), "boom");
        assert_eq!(payload_message(&*formatted), "n=7");
        assert_eq!(payload_message(&*other), "non-string panic");
    }

    #[test]
    fn location_formats_as_file_line_col() {
        let text = format_location(std::panic::Location::caller());
        let mut parts = text.rsplitn(3, ':');
        assert!(parts.next().unwrap().parse::<u32>().is_ok());
        assert!(parts.next().unwrap().parse::<u32>().is_ok());
        assert!(parts.next().unwrap().ends_with("diagnostics.rs"), "{text}");
    }

    #[test]
    fn write_crash_flattens_newlines_in_message() {
        let tmp = tempfile::tempdir().unwrap();
        let path = write_crash(
            tmp.path(),
            CrashSource::App,
            "1.0",
            "line one\nline two",
            "a.rs:1:2",
            "worker",
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("Printdex 1.0 crashed\ntime: "));
        assert!(text.contains("thread: worker\nmessage: line one line two\nlocation: a.rs:1:2\n"));
        assert!(text.contains("\nbacktrace:\n"));
    }

    #[test]
    fn crashes_in_the_same_millisecond_keep_both_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let a = write_crash_file(dir, CRASH_PREFIX, 1234, "first").unwrap();
        let b = write_crash_file(dir, CRASH_PREFIX, 1234, "second").unwrap();
        let c = write_crash_file(dir, CRASH_PREFIX, 1234, "third").unwrap();
        assert_eq!(a.file_name().unwrap(), "crash-1234.txt");
        assert_eq!(b.file_name().unwrap(), "crash-1234-1.txt");
        assert_eq!(c.file_name().unwrap(), "crash-1234-2.txt");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "first");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "second");
        // The newest of the same millisecond comes first.
        let found: Vec<PathBuf> = crash_files(dir, CRASH_PREFIX)
            .into_iter()
            .map(|(_, p)| p)
            .collect();
        assert_eq!(found, [c, b, a]);
    }

    #[test]
    fn cli_crashes_are_bundled_but_raise_no_app_notice() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let cli = write_crash(dir, CrashSource::Cli, "1", "cli boom", "c.rs:1:1", "main").unwrap();
        assert!(
            cli.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("cli-crash-")
        );
        assert!(unseen_crash(dir).is_none());
        write_crash(dir, CrashSource::App, "1", "app boom", "a.rs:1:1", "main").unwrap();
        assert_eq!(unseen_crash(dir).unwrap().message, "app boom");

        let dest = tmp.path().join("bundle.zip");
        save_bundle(dir, &dest, "s", &Redactor::new(None, &[])).unwrap();
        let zip = zip::ZipArchive::new(std::fs::File::open(&dest).unwrap()).unwrap();
        let names: Vec<String> = zip.file_names().map(|n| n.unwrap().into_owned()).collect();
        assert!(
            names.iter().any(|n| n.starts_with("cli-crash-")),
            "{names:?}"
        );
        assert!(names.iter().any(|n| n.starts_with("crash-")), "{names:?}");
    }

    #[test]
    fn crash_seen_marker_hides_older_crashes() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        write_crash(dir, CrashSource::App, "1", "first", "a.rs:1:1", "t").unwrap();
        assert_eq!(unseen_crash(dir).unwrap().message, "first");
        std::thread::sleep(Duration::from_millis(50));
        mark_crashes_seen(dir).unwrap();
        assert!(unseen_crash(dir).is_none());
        std::thread::sleep(Duration::from_millis(50));
        write_crash(dir, CrashSource::App, "1", "second", "a.rs:2:2", "t").unwrap();
        assert_eq!(unseen_crash(dir).unwrap().message, "second");
    }

    #[test]
    fn redacts_home_and_libraries_any_slash_any_case() {
        let r = Redactor::new(Some(Path::new("C:\\Users\\Max")), &["D:\\Prints".into()]);
        assert_eq!(
            r.apply("c:/users/max/x and D:\\prints\\a.stl"),
            "~/x and <library 1>\\a.stl"
        );
        let r = Redactor::new(Some(Path::new("/home/max")), &[]);
        assert_eq!(r.apply("/home/max/.local"), "~/.local");
        assert_eq!(r.apply("/home/maxwell/x"), "/home/maxwell/x");
        assert_eq!(r.apply("é /home/max"), "é ~");
        assert_eq!(r.apply("see /home/max."), "see ~.");
        assert_eq!(r.apply("/home/max.bak/x"), "/home/max.bak/x");
        let r = Redactor::new(Some(Path::new("C:\\Users\\max")), &["D:\\Prints".into()]);
        assert_eq!(r.apply("in D:\\Prints."), "in <library 1>.");
        // As printed by `{:?}`: every backslash doubled.
        assert_eq!(
            r.apply(r#"Err("C:\\Users\\max\\a b" D:\\Prints\\x)"#),
            r#"Err("~\\a b" <library 1>\\x)"#
        );
    }

    #[test]
    fn bundle_holds_summary_logs_and_crashes_redacted() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        std::fs::create_dir_all(&dir).unwrap();
        let home = "/home/max";
        for d in 1..=4 {
            let name = format!("printdex.log.2026-11-0{d}");
            std::fs::write(dir.join(name), format!("opened {home}/Prints/{d}")).unwrap();
        }
        for ms in 1..=6 {
            std::fs::write(dir.join(format!("crash-{ms}.txt")), format!("at {home}/x")).unwrap();
        }
        let dest = tmp.path().join("bundle.zip");
        let r = Redactor::new(Some(Path::new(home)), &[]);
        save_bundle(&dir, &dest, &format!("summary {home}"), &r).unwrap();

        let mut zip = zip::ZipArchive::new(std::fs::File::open(&dest).unwrap()).unwrap();
        let mut names: Vec<String> = zip.file_names().map(|n| n.unwrap().into_owned()).collect();
        names.sort();
        assert_eq!(names.len(), 1 + 3 + 5, "{names:?}");
        assert!(names.contains(&"summary.txt".to_string()));
        assert!(!names.contains(&"crash-1.txt".to_string()));
        assert!(!names.contains(&"printdex.log.2026-11-01".to_string()));
        for name in &names {
            let mut s = String::new();
            zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
            assert!(!s.contains(home), "{name}: {s}");
            assert!(s.contains('~'), "{name}: {s}");
        }
    }

    #[test]
    fn system_summary_lists_parts() {
        let s = system_summary("1.2.3", "software");
        assert!(s.contains("Printdex 1.2.3") && s.contains("renderer: software"));
        assert!(s.contains(std::env::consts::OS));
    }
}
