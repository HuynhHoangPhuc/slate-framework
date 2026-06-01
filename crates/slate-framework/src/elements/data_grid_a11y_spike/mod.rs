//! **Throwaway** 2D data grid — the S0 DataGrid accessibility spike.
//!
//! Purpose: validate the hardest screen-reader surface end-to-end against real
//! VoiceOver — `grid` / `row` / `gridcell` / `columnheader` roles, zero-based
//! cell indices, and 2D arrow-key navigation — *before* designing the real P5
//! `DataGrid`. This element is intentionally minimal and **not part of the
//! public widget set**: no re-export from `elements`, no docs guarantee. The
//! verified role/nav pattern it produces seeds the real widget; then this file
//! is deleted.
//!
//! ## How VoiceOver follows the active cell
//!
//! Roving focus: the grid container is the tab stop; each arrow key moves real
//! Slate focus to the neighbouring cell via [`EventCtx::request_focus`]. The
//! redraw loop feeds `FocusRegistry::focused()` into the macOS adapter's
//! `TreeUpdate::focus`, so VoiceOver announces whatever cell is focused — no
//! grid-specific code in the render hook.
//!
//! ## State is caller-owned
//!
//! The active `(row, col)` lives in a caller-owned [`Signal`] (Strategy A): the
//! constructor reads it under the render observer so a key-driven `set` rebuilds
//! the view; the key handler writes it. Same discipline as `ScrollView::offset`.

mod prepaint;

use slate_renderer::Lpx;
use slate_renderer::scene::RectInstance;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{Element, IntoElement, Sealed};
use crate::reactive::Signal;
use crate::style::Style;
use crate::types::{Bounds, ElementId, LayoutId, NodeContext};

/// Logical-pixel size of one grid cell.
pub(super) const CELL_W: f32 = 120.0;
pub(super) const CELL_H: f32 = 36.0;

/// A throwaway, keyboard-navigable 2D grid for the S0 a11y spike.
///
/// `rows` counts data rows; an extra header row (index 0, `ColumnHeader` cells)
/// is prepended, so the grid has `rows + 1` total rows.
pub struct DataGridA11ySpike {
    /// Number of data rows (excludes the synthesized header row).
    data_rows: usize,
    /// Number of columns.
    cols: usize,
    /// Caller-owned active cell `(row, col)`, in *total* grid coordinates
    /// (row 0 = header). Drives focus target + highlight.
    active: Signal<(usize, usize)>,
    /// `active` read under the render observer at construction (subscribes the
    /// view so a handler's `set` re-renders). Used by prepaint/paint.
    active_value: (usize, usize),
    /// Stable id allocated during prepaint (available afterwards).
    last_id: Option<ElementId>,
}

/// Build a throwaway spike grid with `data_rows` × `cols` data cells plus a
/// header row. `active` is the caller-owned focused-cell signal.
pub fn data_grid_a11y_spike(
    data_rows: usize,
    cols: usize,
    active: Signal<(usize, usize)>,
) -> DataGridA11ySpike {
    let active_value = active.get(); // subscribe the render observer (Strategy A)
    DataGridA11ySpike {
        data_rows,
        cols,
        active,
        active_value,
        last_id: None,
    }
}

impl DataGridA11ySpike {
    /// Total grid rows including the header row at index 0.
    pub(super) fn total_rows(&self) -> usize {
        self.data_rows + 1
    }

    /// Absolute bounds of cell `(row, col)` given the grid's origin.
    pub(super) fn cell_bounds(&self, grid: Bounds, row: usize, col: usize) -> Bounds {
        Bounds::from_origin_size(
            grid.origin.x + col as f32 * CELL_W,
            grid.origin.y + row as f32 * CELL_H,
            CELL_W,
            CELL_H,
        )
    }

    /// Label text / a11y label for a cell. Header cells name the column.
    pub(super) fn cell_label(&self, row: usize, col: usize) -> String {
        if row == 0 {
            format!("Column {}", col + 1)
        } else {
            format!("R{row}C{}", col + 1)
        }
    }
}

impl Element for DataGridA11ySpike {
    type LayoutState = ();
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        // Fixed-size leaf: the whole grid is one box, cells are subdivided by
        // arithmetic in prepaint/paint (no child layout nodes).
        let w = self.cols as f32 * CELL_W;
        let h = self.total_rows() as f32 * CELL_H;
        let style = Style::default().width(w).height(h);
        let node_id = cx
            .taffy
            .new_leaf(taffy::Style::from(&style))
            .unwrap_or_else(|e| {
                log::error!("DataGridA11ySpike: new_leaf failed ({e})");
                taffy::NodeId::from(u64::MAX)
            });
        if let Err(e) = cx.taffy.set_node_context(node_id, Some(NodeContext::None)) {
            log::error!("DataGridA11ySpike: set_node_context failed: {e}");
        }
        (LayoutId(node_id), ())
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        prepaint::build(self, bounds, cx);
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        // Minimal visuals: a filled rect per cell, header + active highlighted.
        // The screen-reader content lives in the a11y tree, not the pixels.
        for row in 0..self.total_rows() {
            for col in 0..self.cols {
                let cb = self.cell_bounds(bounds, row, col);
                let color = if (row, col) == self.active_value {
                    [0.20, 0.45, 0.85, 1.0] // active
                } else if row == 0 {
                    [0.16, 0.16, 0.20, 1.0] // header
                } else {
                    [0.11, 0.11, 0.14, 1.0] // data
                };
                cx.scene.push_rect(RectInstance {
                    rect: [
                        Lpx(cb.origin.x + 1.0),
                        Lpx(cb.origin.y + 1.0),
                        Lpx(cb.size.width - 2.0),
                        Lpx(cb.size.height - 2.0),
                    ],
                    color,
                    corner_radius: Lpx(3.0),
                    _pad: [0.0; 3],
                });
            }
        }
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for DataGridA11ySpike {}

impl IntoElement for DataGridA11ySpike {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
