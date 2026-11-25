//! Small display helpers.

/// `12345` → `"12,345"` (English digit grouping).
pub fn group(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The folder part of a library-relative path: `Minis/Dragons/Red` →
/// `Minis/Dragons`; a top-level entry has none.
pub fn parent_path(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// A file size in decimal units with one decimal: `4_200_000` → `"4.2 MB"`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    // Display only: precision loss above 2^53 bytes does not matter.
    #[allow(clippy::cast_precision_loss)]
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 999.95 && unit < UNITS.len() - 1 {
        v /= 1000.0;
        unit += 1;
    }
    format!(
        "{v:.1} {}",
        UNITS.get(unit).or(UNITS.last()).unwrap_or(&"B")
    )
}

/// A float as a `u32`, saturating: negative and NaN give 0, too large gives
/// `u32::MAX`. Fractions are truncated.
pub fn to_u32(v: f32) -> u32 {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "float-to-int `as` saturates (NaN is 0); that is the wanted result"
    )]
    let n = v as u32;
    n
}

/// A length in physical pixels from a float: rounded, saturating and at
/// least 1, so a degenerate window size can never make a zero-sized texture.
pub fn px(v: f32) -> u32 {
    to_u32(v.round()).max(1)
}

/// A calendar date (UTC) from unix milliseconds: `(year, month 1–12, day)`.
pub fn civil_date(unix_ms: i64) -> (i64, u32, u32) {
    // Howard Hinnant's days-to-civil algorithm.
    let days = unix_ms.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    // Both are small and positive by construction.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (year, month as u32, day as u32)
}

/// The last segment of a `/`-separated path.
pub fn file_name(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, name)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands() {
        assert_eq!(group(0), "0");
        assert_eq!(group(999), "999");
        assert_eq!(group(1_000), "1,000");
        assert_eq!(group(12_345), "12,345");
        assert_eq!(group(1_234_567), "1,234,567");
    }

    #[test]
    fn sizes_in_decimal_units() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1_000), "1.0 KB");
        assert_eq!(size(4_200_000), "4.2 MB");
        assert_eq!(size(999_990), "1.0 MB");
        assert_eq!(size(12_800_000_000), "12.8 GB");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        // 2026-10-03T12:00:00Z
        assert_eq!(civil_date(1_791_028_800_000), (2026, 10, 3));
        // 2024-02-29T00:00:00Z
        assert_eq!(civil_date(1_709_164_800_000), (2024, 2, 29));
        assert_eq!(civil_date(-1), (1969, 12, 31));
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name("a/b/c.stl"), "c.stl");
        assert_eq!(file_name("c.stl"), "c.stl");
    }

    #[test]
    fn parent_path_of_nested_and_top_level() {
        assert_eq!(parent_path("Minis/Dragons/Red"), "Minis/Dragons");
        assert_eq!(parent_path("Red"), "");
        assert_eq!(parent_path("Minis/x.stl"), "Minis");
    }

    #[test]
    fn sizes_never_run_off_the_unit_table() {
        assert_eq!(size(u64::MAX), "18446744.1 TB");
    }

    #[test]
    fn float_sizes_saturate() {
        assert_eq!(to_u32(-5.0), 0);
        assert_eq!(to_u32(f32::NAN), 0);
        assert_eq!(to_u32(2.9), 2);
        assert_eq!(to_u32(f32::INFINITY), u32::MAX);
        assert_eq!(to_u32(1e20), u32::MAX);
    }

    #[test]
    fn pixel_sizes_are_at_least_one() {
        assert_eq!(px(0.0), 1);
        assert_eq!(px(-300.0), 1);
        assert_eq!(px(f32::NAN), 1);
        assert_eq!(px(99.6), 100);
        assert_eq!(px(f32::INFINITY), u32::MAX);
    }
}
