//! List-mode geometry: fixed-height rows below a fixed header, virtualized
//! like the grid.

use std::ops::Range;

use crate::grid;

/// One row, including its 1px divider.
pub const ROW_HEIGHT: f32 = 44.0;
/// The column header above the rows (outside the scrolled area).
pub const HEADER_HEIGHT: f32 = 36.0;

/// Rows to build for a viewport at `offset` that is `height` tall.
pub fn visible_rows(offset: f32, height: f32, items: usize) -> Range<usize> {
    grid::visible_range(offset, height, ROW_HEIGHT, items)
}

/// Scroll offset that brings `row` fully into view, if it is not.
pub fn scroll_target(row: usize, offset: f32, height: f32) -> Option<f32> {
    grid::scroll_target(row, offset, height, ROW_HEIGHT, ROW_HEIGHT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_rows_for_list() {
        // 440px shows rows 0..10; one overscan row below, none above row 0.
        assert_eq!(visible_rows(0.0, 440.0, 1000), 0..12);
        // Scrolled by 10 rows: overscan of one above.
        assert_eq!(visible_rows(440.0, 440.0, 1000), 9..22);
        assert_eq!(visible_rows(0.0, 440.0, 3), 0..3);
        assert_eq!(visible_rows(0.0, 440.0, 0), 0..0);
    }

    #[test]
    fn scroll_target_for_list_row() {
        assert_eq!(scroll_target(0, 0.0, 440.0), None);
        // Row 10 starts at 440 and ends at 484: scroll so its bottom shows.
        assert_eq!(scroll_target(10, 0.0, 440.0), Some(44.0));
        // Row 2 above an offset of 200: scroll to its top.
        assert_eq!(scroll_target(2, 200.0, 440.0), Some(88.0));
    }
}
