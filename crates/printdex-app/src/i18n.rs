//! Translations. Every user-visible string goes through [`t`] or [`t_args`].
//!
//! English is the only bundle for now; the OS locale is picked up once a
//! second language exists (architecture design §7).

use std::cell::RefCell;

use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
use unic_langid::langid;

const EN: &str = include_str!("../locales/en/main.ftl");

thread_local! {
    static BUNDLE: RefCell<FluentBundle<FluentResource>> = RefCell::new(english());
}

fn english() -> FluentBundle<FluentResource> {
    let resource = match FluentResource::try_new(EN.to_owned()) {
        Ok(r) => r,
        Err((r, errors)) => {
            tracing::error!(?errors, "English bundle has syntax errors");
            r
        }
    };
    let mut bundle = FluentBundle::new(vec![langid!("en")]);
    // Unicode isolation marks show up as boxes in some fonts; we only ship
    // left-to-right languages so far.
    bundle.set_use_isolating(false);
    if let Err(errors) = bundle.add_resource(resource) {
        tracing::error!(?errors, "English bundle has duplicate keys");
    }
    bundle
}

/// The message for `key`, or the key itself when it is missing.
pub fn t(key: &str) -> String {
    format(key, None)
}

/// The message for `key` with arguments. Use [`Arg::count`] for numbers
/// that pick a plural form.
pub fn t_args(key: &str, args: &[(&str, Arg)]) -> String {
    let mut fa = FluentArgs::new();
    for (name, arg) in args {
        match arg {
            Arg::Str(s) => fa.set(*name, FluentValue::from(s.clone())),
            Arg::Num(n) => fa.set(*name, FluentValue::from(*n)),
        }
    }
    format(key, Some(&fa))
}

/// One argument to a message.
#[derive(Clone, Debug)]
pub enum Arg {
    Str(String),
    Num(f64),
}

impl Arg {
    pub fn str(s: impl Into<String>) -> Arg {
        Arg::Str(s.into())
    }

    /// A count for plural selection; display it through a separate `$n`.
    pub fn count(n: u64) -> Arg {
        // Plural rules only need the magnitude; precision loss above 2^53 is fine.
        #[allow(clippy::cast_precision_loss)]
        Arg::Num(n as f64)
    }
}

fn format(key: &str, args: Option<&FluentArgs>) -> String {
    BUNDLE.with(|b| {
        let bundle = b.borrow();
        let Some(pattern) = bundle.get_message(key).and_then(|m| m.value()) else {
            tracing::warn!(key, "missing translation");
            return key.to_owned();
        };
        let mut errors = Vec::new();
        let out = bundle.format_pattern(pattern, args, &mut errors);
        if !errors.is_empty() {
            tracing::warn!(key, ?errors, "translation formatting errors");
        }
        out.into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn english_bundle_parses() {
        assert!(FluentResource::try_new(EN.to_owned()).is_ok());
        let mut b = FluentBundle::new(vec![langid!("en")]);
        assert!(
            b.add_resource(FluentResource::try_new(EN.to_owned()).unwrap())
                .is_ok()
        );
    }

    #[test]
    fn t_returns_message() {
        assert_eq!(t("app-title"), "Printdex");
    }

    #[test]
    fn t_args_picks_plural_and_shows_formatted_number() {
        let one = t_args(
            "status-models",
            &[("count", Arg::count(1)), ("n", Arg::str("1"))],
        );
        assert_eq!(one, "1 model");
        let many = t_args(
            "status-models",
            &[("count", Arg::count(12_345)), ("n", Arg::str("12,345"))],
        );
        assert_eq!(many, "12,345 models");
    }

    #[test]
    fn missing_key_falls_back_to_key() {
        let key = "no-such-key";
        assert_eq!(t(key), key);
    }

    /// Fails when code uses a key that the English bundle lacks.
    #[test]
    fn every_key_used_in_code_exists() {
        let defined: BTreeSet<String> = EN
            .lines()
            .filter(|l| !l.starts_with(['#', ' ']) && l.contains(" ="))
            .filter_map(|l| l.split(" =").next())
            .map(str::to_owned)
            .collect();
        let mut used = BTreeSet::new();
        collect_keys(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut used,
        );
        assert!(used.len() > 10, "key scan found too little: {used:?}");
        let missing: Vec<_> = used.difference(&defined).collect();
        assert!(
            missing.is_empty(),
            "keys missing from en/main.ftl: {missing:?}"
        );
    }

    fn collect_keys(dir: &Path, out: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_keys(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let src = std::fs::read_to_string(&path).unwrap();
                for marker in ["t(\"", "t_args(\""] {
                    for (i, _) in src.match_indices(marker) {
                        // Skip calls like `hit("x")` that merely end in `t(`.
                        let before = src[..i].chars().next_back();
                        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                            continue;
                        }
                        let rest = &src[i + marker.len()..];
                        let key: String = rest.chars().take_while(|c| *c != '"').collect();
                        if key.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                            && !key.is_empty()
                        {
                            out.insert(key);
                        }
                    }
                }
            }
        }
    }
}
