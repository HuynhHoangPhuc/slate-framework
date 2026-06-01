//! [`DataGrid`] layout pass: builds the **visible** cell content (header row +
//! the data-row window) as presentational `Text` elements and creates the
//! container Taffy node. Cells are positioned by arithmetic at paint, so the
//! cell content nodes exist only to shape glyphs and to be cleaned up with the
//! container — their Taffy positions are ignored.

use crate::context::LayoutCtx;
use crate::element::AnyElement;
use crate::elements::control_paint::presentational_label;
use crate::style::Style;
use crate::types::{LayoutId, NodeContext};

use super::columns::ColumnModel;
use super::DataGrid;

/// Build the windowed cell content + container node for one frame.
pub(super) fn request(grid: &mut DataGrid, cx: &mut LayoutCtx) -> (LayoutId, ()) {
    let cols = ColumnModel::new(&grid.columns);
    let ncols = grid.columns.len();
    let color = grid.style.fg;
    let size = grid.font_size;

    // Total-rows to materialize this frame: the sticky header (0) + the data
    // window. Build a presentational Text per cell so it shapes glyphs but emits
    // no a11y node (the cell node registered in prepaint carries the label).
    let (first, last) = grid.visible_data_window();
    let mut total_rows: Vec<usize> = Vec::with_capacity(1 + (last - first));
    total_rows.push(0);
    total_rows.extend((first..last).map(|d| d + 1));

    let mut cells: Vec<(usize, usize, AnyElement)> = Vec::with_capacity(total_rows.len() * ncols);
    let mut nodes = Vec::with_capacity(total_rows.len() * ncols);
    for &total_row in &total_rows {
        for c in 0..ncols {
            let mut text = presentational_label(grid.cell_text(total_row, c), color, size);
            nodes.push(text.request_layout(cx).0);
            cells.push((total_row, c, text));
        }
    }
    grid.cells = cells;

    let style = Style::default()
        .width(cols.total_width())
        .height(grid.height);
    let node = cx
        .taffy
        .new_with_children(taffy::Style::from(&style), &nodes)
        .unwrap_or_else(|e| {
            log::error!("DataGrid: new_with_children failed ({e})");
            cx.taffy
                .new_leaf(taffy::Style::default())
                .unwrap_or(taffy::NodeId::from(u64::MAX))
        });
    if let Err(e) = cx.taffy.set_node_context(node, Some(NodeContext::None)) {
        log::error!("DataGrid: set_node_context failed: {e}");
    }

    (LayoutId(node), ())
}
