//! Uniform-height windowed virtualization for [`ScrollView`](super::ScrollView).
//!
//! A virtualized ScrollView renders only the rows that fall inside (or just
//! outside) the viewport, so a list of 100k items costs the same per frame as
//! one of 30. Rows are assumed **uniform height**, which lets the visible
//! window be derived arithmetically — no per-row measurement, no extent cache
//! (variable-height support is deferred until a bench proves it needed).
//!
//! # Why the window is picked at layout time
//!
//! The render pipeline is `request_layout → compute_layout → prepaint → paint`.
//! Taffy nodes are created in `request_layout`, *before* the element's final
//! bounds are known. So the window is computed from the scroll offset (resolved
//! under the render observer at builder time) and the ScrollView's **fixed
//! pixel height** read straight off its style. A virtualized ScrollView
//! therefore needs an explicit pixel height; without one it cannot size the
//! window and falls back to rendering every row (see
//! [`resolve_viewport_height`]). Flex/percent-height virtual viewports would
//! need a second layout pass and are out of scope for the uniform-height pass.

use crate::element::AnyElement;
use crate::style::{Length, Style};

/// Extra rows rendered above and below the visible range so a fast scroll (or
/// the ≤1-frame-stale offset) never flashes a blank edge before the next frame.
pub(super) const OVERSCAN: usize = 2;

/// Lazy, uniform-height row source for a virtualized ScrollView.
///
/// `builder` is called once per visible row index each frame; only the rows in
/// the current window are ever built.
pub(super) struct VirtualList {
    /// Total number of rows in the list (the full, un-windowed count).
    pub count: usize,
    /// Fixed logical-pixel height every row is laid out at.
    pub row_height: f32,
    /// Builds the row element for a given absolute index `[0, count)`.
    pub builder: Box<dyn Fn(usize) -> AnyElement>,
}

/// Total scrollable content height for `count` uniform rows, in logical pixels.
pub(super) fn virtual_content_height(count: usize, row_height: f32) -> f32 {
    count as f32 * row_height
}

/// Resolve the viewport height usable for windowing from the ScrollView style.
///
/// Only a fixed pixel height (`Length::Px`) can be known at `request_layout`
/// time; `Auto`/`Percent` heights depend on the parent layout that hasn't run
/// yet, so they return `None` and the caller renders all rows.
pub(super) fn resolve_viewport_height(style: &Style) -> Option<f32> {
    match style.size.height {
        Length::Px(h) if h > 0.0 => Some(h),
        _ => None,
    }
}

/// Compute the half-open visible row window `[first, last)` for the given scroll
/// geometry, padded by [`OVERSCAN`] rows on each side and clamped to
/// `[0, count]`.
///
/// `offset` is the clamped scroll offset (logical px from the top); `viewport_h`
/// is the viewport height; `row_height` the uniform per-row height.
pub(super) fn visible_window(
    offset: f32,
    viewport_h: f32,
    row_height: f32,
    count: usize,
) -> (usize, usize) {
    if count == 0 || row_height <= 0.0 {
        return (0, 0);
    }
    let first_visible = (offset / row_height).floor() as isize;
    // `+1` covers the partial row at the bottom edge of the viewport.
    let rows_in_view = (viewport_h / row_height).ceil() as isize + 1;
    let overscan = OVERSCAN as isize;
    let first = (first_visible - overscan).max(0) as usize;
    let last = (first_visible + rows_in_view + overscan).max(0) as usize;
    let last = last.min(count);
    (first, last.max(first))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_list_yields_empty_window() {
        assert_eq!(visible_window(0.0, 300.0, 30.0, 0), (0, 0));
    }

    #[test]
    fn zero_row_height_is_safe() {
        assert_eq!(visible_window(0.0, 300.0, 0.0, 100), (0, 0));
    }

    #[test]
    fn top_of_list_starts_at_zero_with_overscan_clamped() {
        // offset 0 → first_visible 0; overscan would go negative → clamps to 0.
        // viewport 300 / 30 = 10 rows + 1 partial + 2 overscan = 13.
        let (first, last) = visible_window(0.0, 300.0, 30.0, 1000);
        assert_eq!(first, 0);
        assert_eq!(last, 13);
    }

    #[test]
    fn middle_window_is_offset_and_overscanned_both_sides() {
        // offset 3000 / 30 = first_visible 100. first = 100 - 2 = 98.
        // last = 100 + (300/30 +1) + 2 = 100 + 11 + 2 = 113.
        let (first, last) = visible_window(3000.0, 300.0, 30.0, 1000);
        assert_eq!(first, 98);
        assert_eq!(last, 113);
    }

    #[test]
    fn bottom_of_list_clamps_last_to_count() {
        // Scrolled near the end: last must not exceed count.
        let (first, last) = visible_window(29_700.0, 300.0, 30.0, 1000);
        assert!(last <= 1000);
        assert_eq!(last, 1000);
        assert!(first < last);
    }

    #[test]
    fn content_height_is_count_times_row() {
        assert_eq!(virtual_content_height(1000, 30.0), 30_000.0);
    }

    #[test]
    fn viewport_height_only_from_fixed_px() {
        let fixed = Style::default().height(300.0);
        assert_eq!(resolve_viewport_height(&fixed), Some(300.0));
        // Default (Auto) → None → caller renders all rows.
        assert_eq!(resolve_viewport_height(&Style::default()), None);
    }
}
