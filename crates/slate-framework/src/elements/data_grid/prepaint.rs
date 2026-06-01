//! [`DataGrid`] prepaint: builds the `Grid > Row > ColumnHeader/Cell` a11y
//! subtree, allocates stable per-cell ids, and wires the roving-focus handler.
//!
//! Every logical cell is registered (a11y node + focusable + hit region) so a
//! Page/Home/End jump can move real focus onto any row — see the module-level
//! note on why windowing the *structure* would break roving. Glyph content is
//! still windowed (built in the layout pass); this pass only registers
//! lightweight nodes.

use std::sync::Arc;

use crate::context::PrepaintCtx;
use crate::event::Handlers;
use crate::focus::FocusableEntry;
use crate::hit_test::HitRegion;
use crate::types::{
    AccessibilityAction, AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, ElementId,
};

use super::columns::ColumnModel;
use super::keys::{cell_click_handler, key_handlers, nav_handler, scroll_handler, NavGeometry};
use super::DataGrid;

/// Distinct marker types so `allocate_id` produces non-colliding grid/row/cell
/// ids even at the same `(index, key)` (the id hash includes the `TypeId`).
struct RowMarker;
struct CellMarker;

/// Build the a11y subtree + focus/handler registrations for one frame.
pub(super) fn build(grid: &mut DataGrid, bounds: Bounds, cx: &mut PrepaintCtx) {
    grid.origin = bounds.origin;
    let cols_model = ColumnModel::new(&grid.columns);
    let total_rows = grid.total_rows();
    let ncols = grid.columns.len();

    let grid_id = cx.allocate_id::<DataGrid>();
    grid.last_id = Some(grid_id);
    cx.push_frame(grid_id);

    // Stable ids up front (deterministic order → stable across frames → focus
    // tracking holds) so the nav handler can capture the full matrix.
    let mut row_ids: Vec<ElementId> = Vec::with_capacity(total_rows);
    let mut cell_ids: Vec<Vec<ElementId>> = Vec::with_capacity(total_rows);
    for r in 0..total_rows {
        cx.set_next_key(format!("row-{r}"));
        row_ids.push(cx.allocate_id::<RowMarker>());
        let mut row = Vec::with_capacity(ncols);
        for c in 0..ncols {
            cx.set_next_key(format!("cell-{r}-{c}"));
            row.push(cx.allocate_id::<CellMarker>());
        }
        cell_ids.push(row);
    }
    let cell_ids = Arc::new(cell_ids);
    let geo = NavGeometry {
        total_rows,
        cols: ncols,
        row_h: grid.row_h,
        data_viewport_h: grid.data_viewport_h(),
        max_offset: grid.max_offset(),
    };
    let handler = nav_handler(
        grid.active.clone(),
        grid.offset.clone(),
        grid.selected.clone(),
        cell_ids.clone(),
        geo,
    );

    // Grid container: the tab stop. Registered before cells so its hit region
    // sits behind them; its handler routes the first arrow into a cell. The
    // wheel handler owns data-area scrolling (bubbled up from the cells).
    cx.register_focusable(
        FocusableEntry { id: grid_id, tab_index: 0, focus_ring: false },
        bounds,
        0.0,
    );
    cx.register_key_handlers(grid_id, key_handlers(handler.clone()));
    cx.register_handlers(
        grid_id,
        Handlers {
            on_mouse_scrolled: Some(scroll_handler(grid.offset.clone(), geo.max_offset)),
            ..Default::default()
        },
    );
    cx.register_hit_region(HitRegion::new(grid_id, bounds, 0));

    // Window of data rows currently on-screen — used to gate each cell's focus
    // ring so a cell scrolled out of view (e.g. by the wheel) shows no stray
    // ring outside the clipped viewport.
    let (win_first, win_last) = grid.visible_data_window();

    cx.prepaint_node_open(grid_id, bounds, grid_info(grid, total_rows, ncols));
    for r in 0..total_rows {
        let row_bounds = row_bounds(grid, &cols_model, r);
        cx.prepaint_node_open(row_ids[r], row_bounds, row_info(r));
        for c in 0..ncols {
            let cb = grid.cell_bounds(&cols_model, r, c);
            let id = cell_ids[r][c];
            let label = grid.cell_text(r, c).to_string();
            cx.register_a11y_node(cell_node(id, cb, label, r, c));
            // Focusable but out of the Tab cycle (-1) — reached via roving / click.
            // Ring only when on-screen (header row 0 is sticky → always shown).
            // Data row `r` (1-based total) maps to data index `r-1`; on-screen
            // when that index is in `[win_first, win_last)`.
            let on_screen = r == 0 || ((win_first + 1)..(win_last + 1)).contains(&r);
            cx.register_focusable(FocusableEntry { id, tab_index: -1, focus_ring: on_screen }, cb, 3.0);
            cx.register_key_handlers(id, key_handlers(handler.clone()));
            cx.register_handlers(
                id,
                cell_click_handler(r, c, id, grid.active.clone(), grid.selected.clone()),
            );
            cx.register_hit_region(HitRegion::new(id, cb, 0));
        }
        cx.prepaint_node_close(); // row
    }
    cx.prepaint_node_close(); // grid

    // Prepaint the windowed cell content (presentational labels emit no a11y
    // node, but the ElementState wrapper requires prepaint before paint).
    // Precompute bounds first to release the immutable borrow before the
    // mutable cell iteration.
    let content: Vec<Bounds> = grid
        .cells
        .iter()
        .map(|(r, c, _)| grid.cell_content_bounds(&cols_model, *r, *c))
        .collect();
    for ((.., elem), cb) in grid.cells.iter_mut().zip(content) {
        elem.prepaint(cb, cx);
    }

    cx.pop_frame();
}

/// Full-width bounds of total-row `r` at its current (scrolled) position.
fn row_bounds(grid: &DataGrid, cols: &ColumnModel, r: usize) -> Bounds {
    let any = grid.cell_bounds(cols, r, 0);
    Bounds::from_origin_size(grid.origin.x, any.origin.y, cols.total_width(), grid.row_h)
}

fn grid_info(grid: &DataGrid, total_rows: usize, cols: usize) -> AccessibilityInfo {
    AccessibilityInfo {
        role: AccessibilityRole::Grid,
        label: grid.label.clone(),
        row_count: Some(total_rows),
        column_count: Some(cols),
        ..Default::default()
    }
}

fn row_info(row: usize) -> AccessibilityInfo {
    AccessibilityInfo {
        role: AccessibilityRole::Row,
        row_index: Some(row),
        ..Default::default()
    }
}

fn cell_node(id: ElementId, bounds: Bounds, label: String, row: usize, col: usize) -> AccessibilityNode {
    let role = if row == 0 {
        AccessibilityRole::ColumnHeader
    } else {
        AccessibilityRole::Cell
    };
    AccessibilityNode {
        id,
        bounds,
        info: AccessibilityInfo {
            role,
            label: Some(label),
            row_index: Some(row),
            column_index: Some(col),
            ..Default::default()
        },
        children: Vec::new(),
        actions: vec![AccessibilityAction::Focus, AccessibilityAction::Click],
    }
}
