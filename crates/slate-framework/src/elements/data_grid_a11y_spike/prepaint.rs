//! Prepaint for the throwaway DataGrid a11y spike: builds the `Grid > Row >
//! Cell` accessibility subtree, allocates stable per-cell ids, and wires the
//! roving-focus arrow-key handler. Kept separate from `mod.rs` to stay <200 LOC.

use std::sync::Arc;

use slate_platform::{Key, NamedKey};

use crate::context::PrepaintCtx;
use crate::event::{ElementKeyHandler, KeyHandlers};
use crate::focus::FocusableEntry;
use crate::hit_test::HitRegion;
use crate::types::{
    AccessibilityAction, AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, ElementId,
};

use super::{CELL_H, CELL_W, DataGridA11ySpike};

/// Distinct marker types so `allocate_id` produces non-colliding row vs cell
/// ids even at the same `(index, key)` (the id hash includes the `TypeId`).
struct RowMarker;
struct CellMarker;

/// Build the a11y subtree + focus/handler registrations for one frame.
pub(super) fn build(grid: &mut DataGridA11ySpike, bounds: Bounds, cx: &mut PrepaintCtx) {
    let total_rows = grid.total_rows();
    let cols = grid.cols;

    let grid_id = cx.allocate_id::<DataGridA11ySpike>();
    grid.last_id = Some(grid_id);

    // Grid is the parent frame for id hashing + event ancestry.
    cx.push_frame(grid_id);

    // Allocate stable ids up front so the nav handler can capture them. Order is
    // deterministic across frames → ids are stable → focus tracking holds.
    let mut row_ids: Vec<ElementId> = Vec::with_capacity(total_rows);
    let mut cell_ids: Vec<Vec<ElementId>> = Vec::with_capacity(total_rows);
    for r in 0..total_rows {
        cx.set_next_key(format!("row-{r}"));
        row_ids.push(cx.allocate_id::<RowMarker>());
        let mut row = Vec::with_capacity(cols);
        for c in 0..cols {
            cx.set_next_key(format!("cell-{r}-{c}"));
            row.push(cx.allocate_id::<CellMarker>());
        }
        cell_ids.push(row);
    }
    let cell_ids = Arc::new(cell_ids);
    let handler = nav_handler(grid.active.clone(), cell_ids.clone(), total_rows, cols);

    // Grid container: the tab stop. Registered before cells so its hit region
    // sits *behind* them (cells win clicks). Its handler routes the first arrow
    // (fired while the grid itself is focused) into the active cell.
    cx.register_focusable(focusable(grid_id, 0), bounds, 3.0);
    cx.register_key_handlers(grid_id, key_handlers(handler.clone()));
    cx.register_hit_region(HitRegion::new(grid_id, bounds, 0));

    // a11y tree: Grid > Row > Cell. Cells are leaves; rows are a11y-only.
    cx.prepaint_node_open(grid_id, bounds, grid_info(total_rows, cols));
    for r in 0..total_rows {
        let row_bounds = Bounds::from_origin_size(
            bounds.origin.x,
            bounds.origin.y + r as f32 * CELL_H,
            cols as f32 * CELL_W,
            CELL_H,
        );
        cx.prepaint_node_open(row_ids[r], row_bounds, row_info(r));
        for c in 0..cols {
            let cb = grid.cell_bounds(bounds, r, c);
            let id = cell_ids[r][c];
            cx.register_a11y_node(cell_node(id, cb, grid.cell_label(r, c), r, c));
            // Focusable but out of the Tab cycle (tab_index -1) — reached only
            // via roving focus / click.
            cx.register_focusable(focusable(id, -1), cb, 3.0);
            cx.register_key_handlers(id, key_handlers(handler.clone()));
            cx.register_hit_region(HitRegion::new(id, cb, 0));
        }
        cx.prepaint_node_close(); // row
    }
    cx.prepaint_node_close(); // grid

    cx.pop_frame();
}

/// Arrow / Home / End / PageUp-Down handler. Reads the active cell, computes the
/// clamped neighbour, updates the caller's signal, and moves real focus there —
/// always, so the first arrow also pulls focus off the grid container onto a
/// cell. `stop_propagation` keeps nav keys from also driving Tab.
fn nav_handler(
    active: crate::reactive::Signal<(usize, usize)>,
    cell_ids: Arc<Vec<Vec<ElementId>>>,
    total_rows: usize,
    cols: usize,
) -> ElementKeyHandler {
    Arc::new(move |ev, cx| {
        let (r, c) = active.get();
        let (nr, nc) = match &ev.key {
            Key::Named(NamedKey::ArrowDown) => ((r + 1).min(total_rows - 1), c),
            Key::Named(NamedKey::ArrowUp) => (r.saturating_sub(1), c),
            Key::Named(NamedKey::ArrowRight) => (r, (c + 1).min(cols - 1)),
            Key::Named(NamedKey::ArrowLeft) => (r, c.saturating_sub(1)),
            Key::Named(NamedKey::Home) => (r, 0),
            Key::Named(NamedKey::End) => (r, cols - 1),
            Key::Named(NamedKey::PageUp) => (0, c),
            Key::Named(NamedKey::PageDown) => (total_rows - 1, c),
            _ => return,
        };
        if (nr, nc) != (r, c) {
            active.set((nr, nc));
        }
        cx.request_focus(cell_ids[nr][nc]);
        cx.stop_propagation();
    })
}

fn focusable(id: ElementId, tab_index: i32) -> FocusableEntry {
    FocusableEntry {
        id,
        tab_index,
        focus_ring: true,
    }
}

fn key_handlers(handler: ElementKeyHandler) -> KeyHandlers {
    KeyHandlers {
        on_key_down: Some(handler),
        ..Default::default()
    }
}

fn grid_info(total_rows: usize, cols: usize) -> AccessibilityInfo {
    AccessibilityInfo {
        role: AccessibilityRole::Grid,
        label: Some("Spike data grid".to_string()),
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
