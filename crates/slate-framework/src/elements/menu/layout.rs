//! Layout + paint for [`MenuList`](super::MenuList): a column of fixed-height,
//! left-padded rows wrapping presentational labels, with an active-row highlight
//! and a selection bar. Split from `mod.rs` to stay under the size budget.

use crate::context::{LayoutCtx, PaintCtx};
use crate::layout::resolve_child_bounds;
use crate::style::{Length, Style};
use crate::types::{Bounds, Edges, LayoutId, NodeContext};

use crate::elements::control_paint::push_rect;
use super::{MenuList, MenuListLayout};

/// Build the column container + one wrapper node per row.
pub(super) fn request(menu: &mut MenuList, cx: &mut LayoutCtx) -> (LayoutId, MenuListLayout) {
    let mut row_nodes = Vec::with_capacity(menu.rows.len());
    for row in menu.rows.iter_mut() {
        let label_id = row.request_layout(cx).0;
        let row_style = Style::new()
            .height(menu.row_h)
            .align_items(taffy::style::AlignItems::Center)
            .padding(Edges::new(
                Length::Px(0.0),
                Length::Px(menu.pad_x),
                Length::Px(0.0),
                Length::Px(menu.pad_x),
            ));
        let node = cx
            .taffy
            .new_with_children(taffy::Style::from(&row_style), &[label_id])
            .unwrap_or_else(|e| {
                log::error!("MenuList: row node failed ({e})");
                cx.taffy.new_leaf(taffy::Style::default()).unwrap_or(taffy::NodeId::from(u64::MAX))
            });
        row_nodes.push(node);
    }

    let column = Style::new().column();
    let container = cx
        .taffy
        .new_with_children(taffy::Style::from(&column), &row_nodes)
        .unwrap_or_else(|e| {
            log::error!("MenuList: container failed ({e})");
            cx.taffy.new_leaf(taffy::Style::default()).unwrap_or(taffy::NodeId::from(u64::MAX))
        });
    if let Err(e) = cx.taffy.set_node_context(container, Some(NodeContext::None)) {
        log::error!("MenuList: set_node_context failed: {e}");
    }
    (LayoutId(container), MenuListLayout { container, rows: row_nodes })
}

/// Paint the active-row highlight, selection bar, and each row label.
pub(super) fn paint(menu: &mut MenuList, bounds: Bounds, layout: &MenuListLayout, cx: &mut PaintCtx) {
    let active = menu.active_value;
    for (i, row) in menu.rows.iter_mut().enumerate() {
        let Some(rb) = resolve_child_bounds(cx.taffy, layout.container, i, bounds.origin) else {
            continue;
        };
        // Active row: faint accent highlight (kept translucent so the label
        // stays readable without recolouring it per frame).
        if i == active && !menu.entries[i].disabled {
            let hl = [menu.accent[0] * 0.3, menu.accent[1] * 0.3, menu.accent[2] * 0.3, 0.3];
            push_rect(cx, rb, hl, 4.0);
        }
        // Committed selection: a solid accent bar at the left edge.
        if menu.selected == Some(i) {
            let bar = Bounds::from_origin_size(
                rb.origin.x,
                rb.origin.y + 4.0,
                3.0,
                (rb.size.height - 8.0).max(0.0),
            );
            push_rect(cx, bar, menu.accent, 1.5);
        }
        if let Some(lb) = resolve_child_bounds(cx.taffy, layout.rows[i], 0, rb.origin) {
            row.paint(lb, cx);
        }
    }
}
