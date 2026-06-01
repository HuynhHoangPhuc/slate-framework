//! Prepaint for [`Tree`](super::Tree): resolves + clamps the roving-focus index,
//! allocates stable per-visible-node ids, wires the shared nav/expand/commit
//! handler, and builds the `Tree` → `TreeItem` accessibility subtree (each item
//! carrying `is_expanded` + Expand/Collapse actions when it has children).

use std::sync::Arc;

use crate::context::PrepaintCtx;
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::layout::resolve_child_bounds;
use crate::types::{
    AccessibilityAction, AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, ElementId,
};

use super::keys::{build_nav, click_handler, key_handlers, roving_handler};
use super::node::VisibleNode;
use super::{Tree, TreeLayout};

/// Distinct marker so visible-node ids never collide with the container id.
struct NodeMarker;

/// Build the focus/handler registrations + a11y subtree for one frame.
pub(super) fn build(tree: &mut Tree, bounds: Bounds, layout: &mut TreeLayout, cx: &mut PrepaintCtx) {
    let n = tree.visible.len();
    let container_id = cx.allocate_id::<Tree>();
    tree.last_id = Some(container_id);

    // Roving active index: transient, clamped to the (possibly just-shrunk by a
    // collapse) visible list so a collapse never strands focus past the end.
    let active = cx.state_registry.use_state::<usize>(container_id, || 0);
    tree.active_value = active.get().min(n.saturating_sub(1));
    if n > 0 && active.get() >= n {
        active.set(tree.active_value);
    }

    cx.push_frame(container_id);

    // Stable per-visible-node ids keyed by the caller's node id, so a node keeps
    // its id (and focus) across expand/collapse rebuilds regardless of position.
    let mut row_ids: Vec<ElementId> = Vec::with_capacity(n);
    for node in &tree.visible {
        cx.set_next_key(format!("node-{}", node.id));
        row_ids.push(cx.allocate_id::<NodeMarker>());
    }
    let row_ids = Arc::new(row_ids);

    let nav_rows: Vec<(u64, usize, bool, bool)> = tree
        .visible
        .iter()
        .map(|v| (v.id, v.depth, v.has_children, v.expanded))
        .collect();
    let nav = Arc::new(build_nav(&nav_rows));
    let handler = roving_handler(
        active,
        tree.expanded.clone(),
        tree.selected.clone(),
        row_ids.clone(),
        nav,
        tree.on_activate.clone(),
    );

    cx.register_focusable(
        FocusableEntry { id: container_id, tab_index: 0, focus_ring: false },
        bounds,
        0.0,
    );
    cx.register_key_handlers(container_id, key_handlers(handler.clone()));
    cx.register_hit_region(HitRegion::new(container_id, bounds, 0));

    cx.prepaint_node_open(container_id, bounds, tree_info(tree.label.as_deref(), n));
    for (i, node) in tree.visible.iter().enumerate() {
        let rb = resolve_child_bounds(cx.taffy, layout.container, i, bounds.origin).unwrap_or(Bounds::ZERO);
        let id = row_ids[i];
        cx.register_a11y_node(tree_item_node(id, rb, node, i));
        cx.register_focusable(FocusableEntry { id, tab_index: -1, focus_ring: true }, rb, 4.0);
        cx.register_key_handlers(id, key_handlers(handler.clone()));
        cx.register_handlers(id, click_handler(node.id, tree.selected.clone(), tree.on_activate.clone()));
        cx.register_hit_region(HitRegion::new(id, rb, 0).with_cursor(CursorStyle::Arrow));
    }
    cx.prepaint_node_close();

    cx.pop_frame();
}

fn tree_info(label: Option<&str>, count: usize) -> AccessibilityInfo {
    AccessibilityInfo {
        role: AccessibilityRole::Tree,
        label: label.map(str::to_string),
        row_count: Some(count),
        ..Default::default()
    }
}

/// `TreeItem` node: name + zero-based visible position, `is_expanded` only for
/// nodes that can expand, and Expand/Collapse actions for those same nodes.
fn tree_item_node(id: ElementId, bounds: Bounds, node: &VisibleNode, row_index: usize) -> AccessibilityNode {
    let mut actions = vec![AccessibilityAction::Focus, AccessibilityAction::Click];
    if node.has_children {
        actions.push(AccessibilityAction::Expand);
        actions.push(AccessibilityAction::Collapse);
    }
    AccessibilityNode {
        id,
        bounds,
        info: AccessibilityInfo {
            role: AccessibilityRole::TreeItem,
            label: Some(node.label.clone()),
            is_selected: Some(node.selected),
            is_expanded: node.has_children.then_some(node.expanded),
            row_index: Some(row_index),
            ..Default::default()
        },
        children: Vec::new(),
        actions,
    }
}
