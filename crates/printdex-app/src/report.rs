//! "Report a bug": the prefilled GitHub issue link and the pieces of the
//! diagnostics summary that come from the running app.

use printdex_core::diagnostics::{CrashReport, Redactor};
use url::form_urlencoded;

const ISSUE_BASE: &str = "https://github.com/wolfedme/printdex/issues/new";
/// Browsers and GitHub both choke on very long links.
const MAX_URL_BYTES: usize = 7_500;
const TITLE_CHARS: usize = 80;

/// The renderer line for the summary. `software` is the viewer's probe;
/// `backend` is the `ICED_BACKEND` environment value, if set.
pub fn renderer_label(software: bool, backend: Option<&str>) -> &'static str {
    if software || backend == Some("tiny-skia") {
        "CPU (software)"
    } else {
        "unknown"
    }
}

/// `printdex-diagnostics-2026-11-23.zip`.
pub fn bundle_name(date: &str) -> String {
    format!("printdex-diagnostics-{date}.zip")
}

fn build(summary: &str, crash: Option<(&str, &str)>) -> String {
    let title = crash
        .map(|(m, _)| format!("Crash: {}", m.chars().take(TITLE_CHARS).collect::<String>()))
        .unwrap_or_default();
    let mut body = format!(
        "### What happened\n\n\n### System\n\n{}\n",
        summary.trim_end()
    );
    if let Some((message, location)) = crash {
        body.push_str(&format!("\n### Last crash\n\n{message}\n{location}\n"));
    }
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("labels", "type: bug")
        .append_pair("title", &title)
        .append_pair("body", &body)
        .finish();
    format!("{ISSUE_BASE}?{query}")
}

/// A new-issue link with the system summary and the last crash (message and
/// location, no backtrace) filled in, every path redacted. The link stays
/// within 7,500 bytes; the crash message is cut first.
pub fn issue_url(summary: &str, crash: Option<&CrashReport>, redactor: &Redactor) -> String {
    let summary = redactor.apply(summary);
    let Some(c) = crash else {
        return build(&summary, None);
    };
    let message = redactor.apply(&c.message);
    let location = redactor.apply(&c.location);
    let chars: Vec<char> = message.chars().collect();
    let attempt = |n: usize| {
        let cut: String = chars.iter().take(n).collect();
        let cut = if n < chars.len() {
            format!("{cut}…")
        } else {
            cut
        };
        build(&summary, Some((&cut, &location)))
    };
    let full = attempt(chars.len());
    if full.len() <= MAX_URL_BYTES {
        return full;
    }
    // Longest prefix that fits (the length only grows with n).
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if attempt(mid).len() <= MAX_URL_BYTES {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    attempt(lo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    const HOME: &str = "/home/max";

    fn redactor() -> Redactor {
        Redactor::new(Some(Path::new(HOME)), &[])
    }

    fn crash(message: &str) -> CrashReport {
        CrashReport {
            path: PathBuf::from("/home/max/crash-1.txt"),
            at_ms: 0,
            message: message.to_owned(),
            location: "src/app.rs:10:5".to_owned(),
        }
    }

    fn query(url: &str) -> std::collections::HashMap<String, String> {
        let parsed = url::Url::parse(url).unwrap();
        parsed.query_pairs().into_owned().collect()
    }

    #[test]
    fn issue_url_prefills_system_and_crash() {
        let url = issue_url(
            "Printdex 1.2.3\nos: linux\n",
            Some(&crash("boom")),
            &redactor(),
        );
        assert!(url.starts_with("https://github.com/wolfedme/printdex/issues/new?"));
        let q = query(&url);
        assert_eq!(q["labels"], "type: bug");
        assert!(q["body"].contains("### What happened"));
        assert!(q["body"].contains("### System"));
        assert!(q["body"].contains("Printdex 1.2.3"));
        assert!(q["body"].contains("### Last crash"));
        assert!(q["body"].contains("src/app.rs:10:5"));
        assert!(q["title"].starts_with("Crash: boom"));
    }

    #[test]
    fn issue_url_never_contains_home() {
        let c = crash("could not open /home/max/prints/a.stl");
        let url = issue_url("home is /home/max\n", Some(&c), &redactor());
        let q = query(&url);
        assert!(q["body"].contains('~'));
        assert!(!q["body"].contains(HOME));
        assert!(!q["title"].contains(HOME));
        assert!(!url.contains("max"));
    }

    #[test]
    fn issue_url_stays_under_limit() {
        let c = crash(&"x".repeat(50_000));
        let url = issue_url("Printdex 1.2.3\n", Some(&c), &redactor());
        assert!(url.len() <= 7_500, "{} bytes", url.len());
        assert!(query(&url)["body"].contains("### System"));
    }

    #[test]
    fn issue_url_without_crash_has_no_crash_section() {
        let url = issue_url("Printdex 1.2.3\n", None, &redactor());
        let q = query(&url);
        assert_eq!(q["title"], "");
        assert!(!q["body"].contains("### Last crash"));
        assert!(q["body"].contains("### System"));
    }

    #[test]
    fn renderer_label_follows_the_probe_and_backend() {
        assert_eq!(renderer_label(true, None), "CPU (software)");
        assert_eq!(renderer_label(false, Some("tiny-skia")), "CPU (software)");
        assert_eq!(renderer_label(false, None), "unknown");
    }

    #[test]
    fn bundle_name_has_the_date() {
        assert_eq!(
            bundle_name("2026-11-23"),
            "printdex-diagnostics-2026-11-23.zip"
        );
    }
}
