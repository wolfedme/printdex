//! Grid geometry and virtualization: pure functions, no widgets.
//!
//! Pane sizes follow DESIGN.md: five columns at the 1280px mock width,
//! 14px column gap, 16px row gap. Panes have fixed heights so the grid can
//! build widgets for visible rows only.

use std::ops::Range;

use crate::theme;

pub const COLUMN_GAP: f32 = 14.0;
pub const ROW_GAP: f32 = 16.0;
pub const MIN_PANE_WIDTH: f32 = 188.0;
/// Pane padding on each side.
pub const PANE_PADDING: f32 = theme::S4;
/// Preview well aspect ratio (height / width).
pub const WELL_RATIO: f32 = 0.75;
/// Most a card's name may take: two lines of body text.
pub const NAME_LINES: f32 = 2.0 * 17.0;
/// Height of the chips (or folder) line on a card.
pub const CHIP_LINE: f32 = 18.0;
/// Height of the last-print line on a card.
pub const STATUS_LINE: f32 = 16.0;
/// Text block under the well: gap, name (up to two lines), gap, chips
/// line, gap, last-print line.
pub const TEXT_BLOCK: f32 = theme::S4 + NAME_LINES + 2.0 * theme::S2 + CHIP_LINE + STATUS_LINE;
/// Rows built above and below the viewport.
pub const OVERSCAN: usize = 1;

/// Layout of the grid for one content width.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridMetrics {
    pub columns: usize,
    pub pane_width: f32,
    pub well_height: f32,
    pub row_height: f32,
}

impl GridMetrics {
    /// Metrics for a content area `width` pixels wide.
    pub fn for_width(width: f32) -> GridMetrics {
        let columns = columns_for_width(width);
        let gaps = COLUMN_GAP * (columns as f32 - 1.0);
        let pane_width = ((width - gaps) / columns as f32).max(1.0);
        let well_height = ((pane_width - 2.0 * PANE_PADDING) * WELL_RATIO).round();
        let row_height = 2.0 * PANE_PADDING + well_height + TEXT_BLOCK;
        GridMetrics {
            columns,
            pane_width,
            well_height,
            row_height,
        }
    }

    /// Distance from one row's top to the next row's top.
    pub fn stride(&self) -> f32 {
        self.row_height + ROW_GAP
    }

    pub fn rows(&self, items: usize) -> usize {
        rows_for_items(items, self.columns)
    }

    /// Rows to build for a viewport at `offset` that is `height` tall.
    pub fn visible_rows(&self, offset: f32, height: f32, items: usize) -> Range<usize> {
        visible_range(offset, height, self.stride(), self.rows(items))
    }

    /// Scroll offset that brings `row` fully into view, if it is not.
    pub fn scroll_target(&self, row: usize, offset: f32, height: f32) -> Option<f32> {
        scroll_target(row, offset, height, self.stride(), self.row_height)
    }
}

/// How many panes fit side by side; at least one.
pub fn columns_for_width(width: f32) -> usize {
    let n = ((width + COLUMN_GAP) / (MIN_PANE_WIDTH + COLUMN_GAP)).floor();
    if n.is_finite() && n >= 1.0 {
        // Truncation is intended: n is a small positive whole number.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = n as usize;
        n
    } else {
        1
    }
}

pub fn rows_for_items(items: usize, columns: usize) -> usize {
    items.div_ceil(columns.max(1))
}

/// Row range covering `[offset, offset + height)` plus [`OVERSCAN`] rows on
/// each side, clamped to `0..rows`.
pub fn visible_range(offset: f32, height: f32, stride: f32, rows: usize) -> Range<usize> {
    if rows == 0 || stride <= 0.0 {
        return 0..0;
    }
    let offset = offset.max(0.0);
    let to_row = |y: f32| -> usize {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let r = (y / stride) as usize;
        r
    };
    let first = to_row(offset).saturating_sub(OVERSCAN).min(rows);
    let last = (to_row((offset + height.max(0.0)).ceil()) + 1 + OVERSCAN).min(rows);
    first..last.max(first)
}

/// Offset that makes `row` fully visible: its top edge when it is above the
/// viewport, its bottom edge when below; `None` when already visible.
pub fn scroll_target(
    row: usize,
    offset: f32,
    height: f32,
    stride: f32,
    row_height: f32,
) -> Option<f32> {
    let top = row as f32 * stride;
    let bottom = top + row_height;
    if top < offset {
        Some(top)
    } else if bottom > offset + height {
        Some((bottom - height).max(0.0))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_for_width_matches_mock() {
        // 1280px window − 232px sidebar − 2 × 20px content padding.
        assert_eq!(columns_for_width(1008.0), 5);
        assert_eq!(columns_for_width(400.0), 2);
        assert_eq!(columns_for_width(100.0), 1);
        assert_eq!(columns_for_width(0.0), 1);
        assert_eq!(columns_for_width(f32::NAN), 1);
    }

    #[test]
    fn panes_fill_the_row() {
        let m = GridMetrics::for_width(1008.0);
        let used = m.pane_width * 5.0 + COLUMN_GAP * 4.0;
        assert!((used - 1008.0).abs() < 0.01);
        assert!(m.pane_width >= MIN_PANE_WIDTH);
    }

    #[test]
    fn rows_for_items_rounds_up() {
        assert_eq!(rows_for_items(0, 5), 0);
        assert_eq!(rows_for_items(11, 5), 3);
        assert_eq!(rows_for_items(10, 5), 2);
        assert_eq!(rows_for_items(3, 0), 3);
    }

    #[test]
    fn visible_range_at_top() {
        // stride 100, viewport 250 → rows 0,1,2 visible, +1 overscan below.
        assert_eq!(visible_range(0.0, 250.0, 100.0, 50), 0..4);
    }

    #[test]
    fn visible_range_scrolled() {
        // offset 1050 → first visible row 10, last 13; overscan 9..15.
        assert_eq!(visible_range(1050.0, 250.0, 100.0, 50), 9..15);
    }

    #[test]
    fn visible_range_clamps_at_end() {
        assert_eq!(visible_range(4900.0, 250.0, 100.0, 50), 48..50);
        assert_eq!(visible_range(99_999.0, 250.0, 100.0, 50), 50..50);
        assert_eq!(visible_range(0.0, 250.0, 100.0, 0), 0..0);
    }

    #[test]
    fn visible_range_builds_few_rows_for_huge_lists() {
        let m = GridMetrics::for_width(1008.0);
        let r = m.visible_rows(100_000.0, 800.0, 5_000);
        assert!(r.len() <= 8, "{r:?}");
    }

    #[test]
    fn scroll_target_keeps_row_visible() {
        // stride 100, row height 84, viewport 250 at offset 200.
        assert_eq!(scroll_target(3, 200.0, 250.0, 100.0, 84.0), None);
        assert_eq!(scroll_target(1, 200.0, 250.0, 100.0, 84.0), Some(100.0));
        assert_eq!(scroll_target(5, 200.0, 250.0, 100.0, 84.0), Some(334.0));
    }
}
