//! Layout + paint for [`Tree`](super::Tree): a column of fixed-height rows whose
//! left padding grows with depth (the indentation), wrapping a presentational
//! label that carries a leading twisty glyph for expandable nodes. An
//! active-row highlight + a committed-selection bar mirror [`List`](crate::List).

use crate::context::{LayoutCtx, PaintCtx};
use crate::elements::control_paint::push_rect;
use crate::layout::resolve_child_bounds;
use crate::style::{Length, Style};
use crate::types::{Bounds, Edges, LayoutId, NodeContext};

use super::{Tree, TreeLayout};

/// Build the column container + one indented wrapper node per visible row.
pub(super) fn request(tree: &mut Tree, cx: &mut LayoutCtx) -> (LayoutId, TreeLayout) {
    let mut row_nodes = Vec::with_capacity(tree.rows.len());
    for (i, row) in tree.rows.iter_mut().enumerate() {
        let label_id = row.request_layout(cx).0;
        let indent = tree.pad_x + tree.visible[i].depth as f32 * tree.indent;
        let row_style = Style::new()
            .height(tree.row_h)
            .align_items(taffy::style::AlignItems::Center)
            .padding(Edges::new(
                Length::Px(0.0),
                Length::Px(tree.pad_x),
                Length::Px(0.0),
                Length::Px(indent),
            ));
        let node = cx
            .taffy
            .new_with_children(taffy::Style::from(&row_style), &[label_id])
            .unwrap_or_else(|e| {
                log::error!("Tree: row node failed ({e})");
                cx.taffy.new_leaf(taffy::Style::default()).unwrap_or(taffy::NodeId::from(u64::MAX))
            });
        row_nodes.push(node);
    }

    let column = Style::new().column();
    let container = cx
        .taffy
        .new_with_children(taffy::Style::from(&column), &row_nodes)
        .unwrap_or_else(|e| {
            log::error!("Tree: container failed ({e})");
            cx.taffy.new_leaf(taffy::Style::default()).unwrap_or(taffy::NodeId::from(u64::MAX))
        });
    if let Err(e) = cx.taffy.set_node_context(container, Some(NodeContext::None)) {
        log::error!("Tree: set_node_context failed: {e}");
    }
    (LayoutId(container), TreeLayout { container, rows: row_nodes })
}

/// Paint the active-row highlight, selection bar, and each (twisty + label) row.
pub(super) fn paint(tree: &mut Tree, bounds: Bounds, layout: &TreeLayout, cx: &mut PaintCtx) {
    let active = tree.active_value;
    for (i, row) in tree.rows.iter_mut().enumerate() {
        let Some(rb) = resolve_child_bounds(cx.taffy, layout.container, i, bounds.origin) else {
            continue;
        };
        if i == active {
            let hl = [tree.accent[0] * 0.3, tree.accent[1] * 0.3, tree.accent[2] * 0.3, 0.3];
            push_rect(cx, rb, hl, 4.0);
        }
        if tree.visible[i].selected {
            let bar = Bounds::from_origin_size(
                rb.origin.x,
                rb.origin.y + 4.0,
                3.0,
                (rb.size.height - 8.0).max(0.0),
            );
            push_rect(cx, bar, tree.accent, 1.5);
        }
        if let Some(lb) = resolve_child_bounds(cx.taffy, layout.rows[i], 0, rb.origin) {
            row.paint(lb, cx);
        }
    }
}
