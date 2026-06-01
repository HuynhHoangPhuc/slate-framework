//! Layout + paint for [`List`](super::List): a column of fixed-height,
//! left-padded rows wrapping presentational labels, with an active-row
//! (roving-focus) highlight and a committed-selection bar — the same row
//! treatment as [`MenuList`](crate::MenuList).

use crate::context::{LayoutCtx, PaintCtx};
use crate::elements::control_paint::push_rect;
use crate::layout::resolve_child_bounds;
use crate::style::{Length, Style};
use crate::types::{Bounds, Edges, LayoutId, NodeContext};

use super::{List, ListLayout};

/// Build the column container + one wrapper node per row.
pub(super) fn request(list: &mut List, cx: &mut LayoutCtx) -> (LayoutId, ListLayout) {
    let mut row_nodes = Vec::with_capacity(list.rows.len());
    for row in list.rows.iter_mut() {
        let label_id = row.request_layout(cx).0;
        let row_style = Style::new()
            .height(list.row_h)
            .align_items(taffy::style::AlignItems::Center)
            .padding(Edges::new(
                Length::Px(0.0),
                Length::Px(list.pad_x),
                Length::Px(0.0),
                Length::Px(list.pad_x),
            ));
        let node = cx
            .taffy
            .new_with_children(taffy::Style::from(&row_style), &[label_id])
            .unwrap_or_else(|e| {
                log::error!("List: row node failed ({e})");
                cx.taffy.new_leaf(taffy::Style::default()).unwrap_or(taffy::NodeId::from(u64::MAX))
            });
        row_nodes.push(node);
    }

    let column = Style::new().column();
    let container = cx
        .taffy
        .new_with_children(taffy::Style::from(&column), &row_nodes)
        .unwrap_or_else(|e| {
            log::error!("List: container failed ({e})");
            cx.taffy.new_leaf(taffy::Style::default()).unwrap_or(taffy::NodeId::from(u64::MAX))
        });
    if let Err(e) = cx.taffy.set_node_context(container, Some(NodeContext::None)) {
        log::error!("List: set_node_context failed: {e}");
    }
    (LayoutId(container), ListLayout { container, rows: row_nodes })
}

/// Paint the active-row highlight, selection bar, and each row label.
pub(super) fn paint(list: &mut List, bounds: Bounds, layout: &ListLayout, cx: &mut PaintCtx) {
    let active = list.active_value;
    for (i, row) in list.rows.iter_mut().enumerate() {
        let Some(rb) = resolve_child_bounds(cx.taffy, layout.container, i, bounds.origin) else {
            continue;
        };
        // Active (roving-focus) row: faint translucent accent highlight, kept
        // light so the label stays readable without recolouring it per frame.
        if i == active && !list.entries[i].disabled {
            let hl = [list.accent[0] * 0.3, list.accent[1] * 0.3, list.accent[2] * 0.3, 0.3];
            push_rect(cx, rb, hl, 4.0);
        }
        // Committed selection: a solid accent bar at the left edge.
        if list.selected_value == Some(i) {
            let bar = Bounds::from_origin_size(
                rb.origin.x,
                rb.origin.y + 4.0,
                3.0,
                (rb.size.height - 8.0).max(0.0),
            );
            push_rect(cx, bar, list.accent, 1.5);
        }
        if let Some(lb) = resolve_child_bounds(cx.taffy, layout.rows[i], 0, rb.origin) {
            row.paint(lb, cx);
        }
    }
}
