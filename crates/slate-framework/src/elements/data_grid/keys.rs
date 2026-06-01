//! Keyboard + click handlers for [`DataGrid`].
//!
//! One roving-focus key handler is shared by the grid container and every cell
//! (verbatim from the S0 spike): it reads the caller-owned active `(row, col)`,
//! computes the clamped neighbour, writes the signal, scrolls the target row
//! into view, and moves **real** focus to the neighbour cell — always, so the
//! first arrow also pulls focus off the grid container onto a cell. Enter/Space
//! (and a cell click) commit the optional row selection.

use std::sync::Arc;

use slate_platform::{Key, NamedKey};

use crate::event::{
    ElementKeyHandler, EventCtx, Handlers, KeyEvent, KeyHandlers, MouseEvent, MouseHandler,
    ScrollHandler,
};
use crate::reactive::Signal;
use crate::types::ElementId;

/// Logical pixels scrolled per discrete wheel notch (matches `ScrollView`).
const SCROLL_SPEED: f32 = 40.0;

/// Grid dimensions + scroll geometry captured for the nav handler (≤1 frame
/// stale, like the ScrollView scroll handlers).
#[derive(Clone, Copy)]
pub(super) struct NavGeometry {
    pub total_rows: usize,
    pub cols: usize,
    pub row_h: f32,
    pub data_viewport_h: f32,
    pub max_offset: f32,
}

/// Per-cell id matrix indexed `[total_row][col]` (`total_row 0` = header).
pub(super) type CellIds = Arc<Vec<Vec<ElementId>>>;

/// Clamp `offset` so the data row `data_row` (0-based, header excluded) is fully
/// within the data viewport. Returns the (clamped) offset to apply.
fn scroll_into_view(offset: f32, data_row: usize, geo: &NavGeometry) -> f32 {
    let row_top = data_row as f32 * geo.row_h;
    let row_bottom = row_top + geo.row_h;
    let next = if row_top < offset {
        row_top
    } else if row_bottom > offset + geo.data_viewport_h {
        row_bottom - geo.data_viewport_h
    } else {
        offset
    };
    next.clamp(0.0, geo.max_offset.max(0.0))
}

/// Pure roving target for a navigation key, clamped at the grid edges (no wrap).
/// `None` for any non-navigation key. Verbatim from the S0 spike: PageUp/Down
/// jump to the first/last row, Home/End to the first/last column.
pub(super) fn next_cell(
    key: &Key,
    r: usize,
    c: usize,
    total_rows: usize,
    cols: usize,
) -> Option<(usize, usize)> {
    let last_row = total_rows.saturating_sub(1);
    let last_col = cols.saturating_sub(1);
    Some(match key {
        Key::Named(NamedKey::ArrowDown) => ((r + 1).min(last_row), c),
        Key::Named(NamedKey::ArrowUp) => (r.saturating_sub(1), c),
        Key::Named(NamedKey::ArrowRight) => (r, (c + 1).min(last_col)),
        Key::Named(NamedKey::ArrowLeft) => (r, c.saturating_sub(1)),
        Key::Named(NamedKey::Home) => (r, 0),
        Key::Named(NamedKey::End) => (r, last_col),
        Key::Named(NamedKey::PageUp) => (0, c),
        Key::Named(NamedKey::PageDown) => (last_row, c),
        _ => return None,
    })
}

/// Build the roving navigation + commit key handler shared by the container and
/// every cell.
pub(super) fn nav_handler(
    active: Signal<(usize, usize)>,
    offset: Signal<f32>,
    selected: Option<Signal<Option<usize>>>,
    cell_ids: CellIds,
    geo: NavGeometry,
) -> ElementKeyHandler {
    Arc::new(move |ev: &KeyEvent, cx: &mut EventCtx| {
        let (r, c) = active.get();
        // Enter/Space commit the active data row's selection (not a move).
        if matches!(&ev.key, Key::Named(NamedKey::Enter | NamedKey::Space)) {
            if !ev.is_repeat
                && r > 0
                && let Some(sel) = &selected
            {
                sel.set(Some(r - 1));
            }
            cx.stop_propagation();
            return;
        }
        let Some((nr, nc)) = next_cell(&ev.key, r, c, geo.total_rows, geo.cols) else {
            return;
        };
        if (nr, nc) != (r, c) {
            active.set((nr, nc));
        }
        // Keep the (data) target row visible so the focus ring is on-screen.
        if nr > 0 {
            let next = scroll_into_view(offset.get(), nr - 1, &geo);
            if (next - offset.get()).abs() > f32::EPSILON {
                offset.set(next);
            }
        }
        if let Some(id) = cell_ids.get(nr).and_then(|row| row.get(nc)) {
            cx.request_focus(*id);
        }
        cx.stop_propagation();
    })
}

/// Build the click handler for cell `(total_row, col)`: focuses it, sets the
/// active cell, and commits row selection for data rows.
pub(super) fn cell_click_handler(
    total_row: usize,
    col: usize,
    id: ElementId,
    active: Signal<(usize, usize)>,
    selected: Option<Signal<Option<usize>>>,
) -> Handlers {
    let on_click: MouseHandler = Arc::new(move |_ev: &MouseEvent, cx: &mut EventCtx| {
        active.set((total_row, col));
        if total_row > 0
            && let Some(sel) = &selected
        {
            sel.set(Some(total_row - 1));
        }
        cx.request_focus(id);
        cx.stop_propagation();
    });
    Handlers {
        on_click: Some(on_click),
        ..Default::default()
    }
}

/// Build the wheel/trackpad scroll handler for the data area. `delta_y > 0` =
/// scroll up (content down) = a smaller offset, mirroring `ScrollView`. Bubbles
/// from the cells up to the grid container, which owns this handler.
pub(super) fn scroll_handler(offset: Signal<f32>, max_offset: f32) -> ScrollHandler {
    Arc::new(move |ev, cx| {
        let step = if ev.precise { ev.delta_y } else { ev.delta_y * SCROLL_SPEED };
        let cur = offset.get();
        let next = (cur - step).clamp(0.0, max_offset.max(0.0));
        if (next - cur).abs() > f32::EPSILON {
            offset.set(next);
            cx.stop_propagation();
        }
    })
}

/// Wrap a key handler as the `on_key_down`-only [`KeyHandlers`].
pub(super) fn key_handlers(h: ElementKeyHandler) -> KeyHandlers {
    KeyHandlers {
        on_key_down: Some(h),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geo() -> NavGeometry {
        NavGeometry {
            total_rows: 11, // 1 header + 10 data
            cols: 3,
            row_h: 20.0,
            data_viewport_h: 100.0, // 5 rows visible
            max_offset: 10.0 * 20.0 - 100.0, // 100
        }
    }

    #[test]
    fn scroll_into_view_pages_down_to_reveal_row() {
        // Row 9 (last data row) top=180, bottom=200; viewport 100 → offset 100.
        assert_eq!(scroll_into_view(0.0, 9, &geo()), 100.0);
    }

    #[test]
    fn scroll_into_view_pages_up_to_reveal_row() {
        // Row 0 top=0 < offset 100 → snap offset to 0.
        assert_eq!(scroll_into_view(100.0, 0, &geo()), 0.0);
    }

    #[test]
    fn scroll_into_view_noop_when_already_visible() {
        // Row 2 (top=40,bottom=60) within [0,100] → unchanged.
        assert_eq!(scroll_into_view(0.0, 2, &geo()), 0.0);
    }

    #[test]
    fn scroll_into_view_clamps_to_max() {
        // Even asking for the last row never exceeds max_offset.
        assert!(scroll_into_view(0.0, 9, &geo()) <= geo().max_offset);
    }

    fn nav(key: NamedKey, r: usize, c: usize) -> Option<(usize, usize)> {
        // 11 total rows (1 header + 10 data), 3 cols.
        super::next_cell(&Key::Named(key), r, c, 11, 3)
    }

    #[test]
    fn nav_moves_within_grid() {
        assert_eq!(nav(NamedKey::ArrowDown, 5, 1), Some((6, 1)));
        assert_eq!(nav(NamedKey::ArrowUp, 5, 1), Some((4, 1)));
        assert_eq!(nav(NamedKey::ArrowRight, 5, 1), Some((5, 2)));
        assert_eq!(nav(NamedKey::ArrowLeft, 5, 1), Some((5, 0)));
    }

    #[test]
    fn nav_clamps_at_edges_no_wrap() {
        assert_eq!(nav(NamedKey::ArrowUp, 0, 0), Some((0, 0)), "up at top header holds");
        assert_eq!(nav(NamedKey::ArrowDown, 10, 0), Some((10, 0)), "down at last row holds");
        assert_eq!(nav(NamedKey::ArrowLeft, 3, 0), Some((3, 0)), "left at col 0 holds");
        assert_eq!(nav(NamedKey::ArrowRight, 3, 2), Some((3, 2)), "right at last col holds");
    }

    #[test]
    fn nav_home_end_page_jump_to_extremes() {
        assert_eq!(nav(NamedKey::Home, 4, 2), Some((4, 0)), "Home → first column");
        assert_eq!(nav(NamedKey::End, 4, 0), Some((4, 2)), "End → last column");
        assert_eq!(nav(NamedKey::PageUp, 4, 1), Some((0, 1)), "PageUp → header row");
        assert_eq!(nav(NamedKey::PageDown, 4, 1), Some((10, 1)), "PageDown → last row");
    }

    #[test]
    fn nav_ignores_non_navigation_keys() {
        assert_eq!(nav(NamedKey::Enter, 4, 1), None);
        assert_eq!(nav(NamedKey::Tab, 4, 1), None);
    }
}
