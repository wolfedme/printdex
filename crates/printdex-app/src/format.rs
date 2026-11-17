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
    fn parent_path_of_nested_and_top_level() {
        assert_eq!(parent_path("Minis/Dragons/Red"), "Minis/Dragons");
        assert_eq!(parent_path("Red"), "");
        assert_eq!(parent_path("Minis/x.stl"), "Minis");
    }
}
