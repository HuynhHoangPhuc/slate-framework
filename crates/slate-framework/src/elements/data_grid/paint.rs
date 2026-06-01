//! [`DataGrid`] paint: a sticky header strip, then the visible data rows clipped
//! to the scroll viewport. The active-cell focus ring is emitted by the
//! framework's focus pass (the active cell is the focused focusable), so this
//! pass only draws backgrounds, the header hairline, and cell text.

use slate_renderer::Lpx;
use slate_renderer::scene::ClipRect;

use crate::context::PaintCtx;
use crate::elements::control_paint::push_rect;
use crate::types::Bounds;

use super::columns::ColumnModel;
use super::DataGrid;

/// Paint the grid for one frame.
pub(super) fn render(grid: &mut DataGrid, bounds: Bounds, cx: &mut PaintCtx) {
    grid.origin = bounds.origin;
    let cols = ColumnModel::new(&grid.columns);
    let total_w = cols.total_width();
    let style = grid.style;
    let row_h = grid.row_h;
    let pad_x = grid.pad_x;
    let data_h = grid.data_viewport_h();
    let selected = grid.selected_value;

    // Precompute each cell's text bounds (immutable borrow) before painting the
    // cell elements (mutable borrow).
    let content: Vec<Bounds> = grid
        .cells
        .iter()
        .map(|(r, c, _)| {
            let cb = grid.cell_bounds(&cols, *r, *c);
            let y = cb.origin.y + (row_h - grid.font_size) * 0.5;
            Bounds::from_origin_size(
                cb.origin.x + pad_x,
                y,
                (cb.size.width - 2.0 * pad_x).max(0.0),
                grid.font_size,
            )
        })
        .collect();

    // Header strip (sticky, above the clip) + bottom hairline.
    push_rect(cx, Bounds::from_origin_size(bounds.origin.x, bounds.origin.y, total_w, row_h), style.header_bg, 0.0);
    push_rect(
        cx,
        Bounds::from_origin_size(bounds.origin.x, bounds.origin.y + row_h - 1.0, total_w, 1.0),
        style.border,
        0.0,
    );

    // Data area: clip to the viewport below the header, then paint the selected
    // row tint and the windowed data cell text.
    cx.push_clip(ClipRect::new(
        Lpx(bounds.origin.x),
        Lpx(bounds.origin.y + row_h),
        Lpx(total_w),
        Lpx(data_h),
    ));
    if let Some(sel) = selected {
        let y = bounds.origin.y + row_h + sel as f32 * row_h - grid.offset_value;
        push_rect(cx, Bounds::from_origin_size(bounds.origin.x, y, total_w, row_h), style.sel_bg, 0.0);
    }
    for (i, (r, _, elem)) in grid.cells.iter_mut().enumerate() {
        if *r > 0 {
            elem.paint(content[i], cx);
        }
    }
    cx.pop_clip();

    // Header cell text on top of the header strip.
    for (i, (r, _, elem)) in grid.cells.iter_mut().enumerate() {
        if *r == 0 {
            elem.paint(content[i], cx);
        }
    }
}
