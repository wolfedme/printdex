//! Filter chips: what the user picked under "+ Filter", turned into the
//! engine's [`SearchFilter`].

use printdex_core::{PrintableFormat, SearchFilter};

/// One active filter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    /// Models with at least one part in this format. Several format chips
    /// match any of them.
    Format(PrintableFormat),
    /// Single-file models (`false`) or multi-part models (`true`).
    MultiPart(bool),
}

/// Every chip the filter menu offers, in menu order.
pub const MENU: [Chip; 5] = [
    Chip::Format(PrintableFormat::Stl),
    Chip::Format(PrintableFormat::ThreeMf),
    Chip::Format(PrintableFormat::Obj),
    Chip::MultiPart(false),
    Chip::MultiPart(true),
];

/// Adds `chip`, or removes it when it is already active. The two parts
/// chips exclude each other.
pub fn toggle(chips: &mut Vec<Chip>, chip: Chip) {
    if let Some(i) = chips.iter().position(|c| *c == chip) {
        chips.remove(i);
        return;
    }
    if let Chip::MultiPart(_) = chip {
        chips.retain(|c| !matches!(c, Chip::MultiPart(_)));
    }
    chips.push(chip);
}

/// The engine filter for `chips`.
pub fn to_filter(chips: &[Chip]) -> SearchFilter {
    let mut filter = SearchFilter::default();
    for chip in chips {
        match *chip {
            Chip::Format(f) => {
                if !filter.formats.contains(&f) {
                    filter.formats.push(f);
                }
            }
            Chip::MultiPart(m) => filter.multi_part = Some(m),
        }
    }
    filter
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chips_to_filter() {
        let chips = [
            Chip::Format(PrintableFormat::Stl),
            Chip::Format(PrintableFormat::Obj),
            Chip::MultiPart(true),
        ];
        let f = to_filter(&chips);
        assert_eq!(f.formats, vec![PrintableFormat::Stl, PrintableFormat::Obj]);
        assert_eq!(f.multi_part, Some(true));
        assert!(to_filter(&[]).is_empty());
    }

    #[test]
    fn removing_last_chip_clears_filter() {
        let mut chips = Vec::new();
        toggle(&mut chips, Chip::Format(PrintableFormat::ThreeMf));
        assert!(!to_filter(&chips).is_empty());
        toggle(&mut chips, Chip::Format(PrintableFormat::ThreeMf));
        assert!(chips.is_empty());
        assert!(to_filter(&chips).is_empty());
    }

    #[test]
    fn parts_chips_exclude_each_other() {
        let mut chips = vec![Chip::Format(PrintableFormat::Stl)];
        toggle(&mut chips, Chip::MultiPart(false));
        toggle(&mut chips, Chip::MultiPart(true));
        assert_eq!(
            chips,
            vec![Chip::Format(PrintableFormat::Stl), Chip::MultiPart(true)]
        );
    }
}
