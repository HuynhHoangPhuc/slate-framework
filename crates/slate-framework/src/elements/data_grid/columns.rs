//! Column model + cell-bounds / visible-window arithmetic for [`DataGrid`].
//!
//! A grid lays its cells out by arithmetic (like the S0 spike), not via Taffy
//! child nodes, so the column model is just prefix-summed x offsets and a
//! uniform row height. The visible-row window mirrors
//! [`ScrollView::virtualized`](crate::ScrollView)'s uniform-height windowing so
//! only on-screen rows pay for glyph shaping.

use crate::types::Bounds;

use super::DataGrid;

/// Extra data rows materialized above/below the viewport so a fast scroll never
/// flashes a blank edge before the next frame. Matches the ScrollView overscan.
const OVERSCAN: usize = 2;

/// A grid column: a header label plus a fixed logical-pixel width.
#[derive(Clone, Debug)]
pub struct Column {
    /// Header-cell text + the column's accessible name.
    pub label: String,
    /// Column width in logical pixels.
    pub width: f32,
}

impl Column {
    /// A column with `label` and `width` (logical px).
    pub fn new(label: impl Into<String>, width: f32) -> Self {
        Self {
            label: label.into(),
            width: width.max(0.0),
        }
    }
}

/// Prefix-summed column x-offsets, derived once per frame from the columns.
pub(super) struct ColumnModel {
    /// Left edge (logical px, relative to grid origin) of each column.
    x: Vec<f32>,
    /// Per-column width.
    w: Vec<f32>,
    /// Total of all column widths.
    total: f32,
}

impl ColumnModel {
    /// Build the prefix-sum model from the column list.
    pub(super) fn new(columns: &[Column]) -> Self {
        let mut x = Vec::with_capacity(columns.len());
        let mut acc = 0.0_f32;
        for c in columns {
            x.push(acc);
            acc += c.width;
        }
        Self {
            x,
            w: columns.iter().map(|c| c.width).collect(),
            total: acc,
        }
    }

    /// Total grid width (sum of column widths).
    pub(super) fn total_width(&self) -> f32 {
        self.total
    }

    /// Left edge of column `col` relative to the grid origin (clamped to last).
    pub(super) fn col_x(&self, col: usize) -> f32 {
        self.x.get(col).copied().unwrap_or(self.total)
    }

    /// Width of column `col` (0 if out of range).
    pub(super) fn col_w(&self, col: usize) -> f32 {
        self.w.get(col).copied().unwrap_or(0.0)
    }
}

impl DataGrid {
    /// Total grid rows including the sticky header row at index 0.
    pub(super) fn total_rows(&self) -> usize {
        self.rows.len() + 1
    }

    /// Cell text for `(total_row, col)`: header row names the column; data rows
    /// read the owned table (empty string if ragged/out of range).
    pub(super) fn cell_text(&self, total_row: usize, col: usize) -> &str {
        if total_row == 0 {
            self.columns.get(col).map(|c| c.label.as_str()).unwrap_or("")
        } else {
            self.rows
                .get(total_row - 1)
                .and_then(|r| r.get(col))
                .map(String::as_str)
                .unwrap_or("")
        }
    }

    /// Absolute screen bounds of cell `(total_row, col)` given the grid origin
    /// and current scroll offset. Header (row 0) is pinned; data rows (1+) are
    /// translated up by the scroll offset.
    pub(super) fn cell_bounds(&self, cols: &ColumnModel, total_row: usize, col: usize) -> Bounds {
        let x = self.origin.x + cols.col_x(col);
        let y = if total_row == 0 {
            self.origin.y
        } else {
            self.origin.y + self.row_h + (total_row - 1) as f32 * self.row_h - self.offset_value
        };
        Bounds::from_origin_size(x, y, cols.col_w(col), self.row_h)
    }

    /// Inner text bounds of cell `(total_row, col)`: the cell box inset by
    /// horizontal padding and vertically centred for the font. Shared by
    /// prepaint (to prepaint the label) and paint (to paint it) so the two
    /// passes agree on geometry.
    pub(super) fn cell_content_bounds(&self, cols: &ColumnModel, total_row: usize, col: usize) -> Bounds {
        let cb = self.cell_bounds(cols, total_row, col);
        Bounds::from_origin_size(
            cb.origin.x + self.pad_x,
            cb.origin.y + (self.row_h - self.font_size) * 0.5,
            (cb.size.width - 2.0 * self.pad_x).max(0.0),
            self.font_size,
        )
    }

    /// Height of the scrollable data area below the sticky header.
    pub(super) fn data_viewport_h(&self) -> f32 {
        (self.height - self.row_h).max(0.0)
    }

    /// Max scroll offset: data content height minus the data viewport.
    pub(super) fn max_offset(&self) -> f32 {
        (self.rows.len() as f32 * self.row_h - self.data_viewport_h()).max(0.0)
    }

    /// Half-open window `[first, last)` over **data** rows (0-based, excludes the
    /// header) that need glyph content built this frame, padded by overscan.
    pub(super) fn visible_data_window(&self) -> (usize, usize) {
        data_window(self.offset_value, self.data_viewport_h(), self.row_h, self.rows.len())
    }
}

/// Pure uniform-height windowing: the half-open `[first, last)` data-row range
/// inside (and `OVERSCAN` rows around) a `viewport_h`-tall data area scrolled by
/// `offset`, clamped to `[0, count]`. Mirrors `ScrollView`'s window math.
pub(super) fn data_window(offset: f32, viewport_h: f32, row_h: f32, count: usize) -> (usize, usize) {
    if count == 0 || row_h <= 0.0 {
        return (0, 0);
    }
    let first_visible = (offset / row_h).floor() as isize;
    let rows_in_view = (viewport_h / row_h).ceil() as isize + 1;
    let first = (first_visible - OVERSCAN as isize).max(0) as usize;
    let last = ((first_visible + rows_in_view + OVERSCAN as isize).max(0) as usize).min(count);
    (first, last.max(first))
}

#[cfg(test)]
mod tests {
    use super::{data_window, Column, ColumnModel};

    #[test]
    fn column_model_prefix_sums_widths() {
        let m = ColumnModel::new(&[Column::new("a", 80.0), Column::new("b", 120.0), Column::new("c", 50.0)]);
        assert_eq!(m.total_width(), 250.0);
        assert_eq!(m.col_x(0), 0.0);
        assert_eq!(m.col_x(1), 80.0);
        assert_eq!(m.col_x(2), 200.0);
        assert_eq!(m.col_w(1), 120.0);
        // Out-of-range column clamps to total width / zero.
        assert_eq!(m.col_x(9), 250.0);
        assert_eq!(m.col_w(9), 0.0);
    }

    #[test]
    fn window_empty_or_zero_height_is_empty() {
        assert_eq!(data_window(0.0, 300.0, 20.0, 0), (0, 0));
        assert_eq!(data_window(0.0, 300.0, 0.0, 100), (0, 0));
    }

    #[test]
    fn window_top_clamps_overscan_to_zero() {
        // offset 0 → first_visible 0; 300/20=15 +1 +2 overscan = 18.
        assert_eq!(data_window(0.0, 300.0, 20.0, 1000), (0, 18));
    }

    #[test]
    fn window_middle_offsets_and_overscans_both_sides() {
        // offset 2000/20 = 100. first = 98; last = 100 + (15+1) + 2 = 118.
        assert_eq!(data_window(2000.0, 300.0, 20.0, 1000), (98, 118));
    }

    #[test]
    fn window_bottom_clamps_last_to_count() {
        let (first, last) = data_window(19_700.0, 300.0, 20.0, 1000);
        assert_eq!(last, 1000);
        assert!(first < last);
    }
}
