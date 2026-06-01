//! Conversion from the Slate accessibility contract to `accesskit::Node`.
//!
//! `accesskit::Node` is a *builder* — you call `Node::new(role)` and then
//! mutate via setters. This module is therefore a set of free functions
//! rather than a `From` impl (a `From<&AccessibilityNode> for accesskit::Node`
//! impl clashes ergonomically with the builder shape and the flat-tree pass
//! needed for child references).
//!
//! Children are *not* embedded in `accesskit::Node`; they are referenced by
//! `accesskit::NodeId`. We translate `ElementId` → `accesskit::NodeId` via
//! `ElementId::as_u64()`, which is documented as the stable contract in
//! `docs/a11y-contract.md`.
//!
//! Two entry points:
//!
//! - [`to_accesskit_node`] converts a single `AccessibilityNode` to an
//!   `accesskit::Node`, populating its child list with the NodeIds of its
//!   direct children. Use when the caller has its own flatten loop.
//! - [`to_accesskit_tree`] does a depth-first pass and returns the flat
//!   `Vec<(NodeId, Node)>` an accesskit `TreeUpdate` consumes.
//!
//! Platform AccessKit wiring (E20b) is post-v1; this module only locks the
//! conversion shape so the future platform layer can adopt it unchanged.

use accesskit::{Node, NodeId, Rect, Role, Tree, TreeId, TreeUpdate};

use crate::types::{AccessibilityNode, AccessibilityRelationships, Bounds, ElementId, LiveRegion};

/// Synthetic NodeId for the window root that wraps the view's top-level a11y
/// nodes. Real `ElementId`s come from `allocate_id`'s 64-bit hash, so collision
/// with this `u64::MAX` sentinel is theoretically possible but ~1/2⁶⁴ —
/// negligible, and accepted rather than reserving a value out of the hash space.
pub const WINDOW_ROOT_NODE_ID: NodeId = NodeId(u64::MAX);

/// Translate a Slate `ElementId` to an accesskit `NodeId`.
///
/// The raw `u64` payload is the stable mapping documented in
/// `docs/a11y-contract.md`. Round-trip safe.
#[inline]
pub fn element_id_to_node_id(id: ElementId) -> NodeId {
    NodeId(id.as_u64())
}

/// Convert a single `AccessibilityNode` to an `accesskit::Node`.
///
/// The returned node lists its direct children by `NodeId` only; children
/// themselves must be converted (recursively or via [`to_accesskit_tree`])
/// and submitted alongside in the same `TreeUpdate`.
pub fn to_accesskit_node(node: &AccessibilityNode) -> Node {
    let info = &node.info;
    let mut ak = Node::new(info.role.into());

    if let Some(label) = &info.label {
        ak.set_label(label.clone());
    }
    if let Some(description) = &info.description {
        ak.set_description(description.clone());
    }
    if let Some(value) = &info.value {
        ak.set_value(value.clone());
    }

    if info.is_disabled {
        ak.set_disabled();
    }
    if let Some(expanded) = info.is_expanded {
        ak.set_expanded(expanded);
    }
    if let Some(selected) = info.is_selected {
        ak.set_selected(selected);
    }
    // `is_focused` has no direct accesskit setter on Node — focus is carried
    // by `TreeUpdate::focus`. Documented in `docs/a11y-contract.md`.

    if let Some(live) = info.live_region {
        ak.set_live(match live {
            LiveRegion::Polite => accesskit::Live::Polite,
            LiveRegion::Assertive => accesskit::Live::Assertive,
        });
    }

    // Grid/table cell-position semantics. Row/column counts go on the grid
    // container; row/column indices go on each cell. All zero-based.
    if let Some(rc) = info.row_count {
        ak.set_row_count(rc);
    }
    if let Some(cc) = info.column_count {
        ak.set_column_count(cc);
    }
    if let Some(ri) = info.row_index {
        ak.set_row_index(ri);
    }
    if let Some(ci) = info.column_index {
        ak.set_column_index(ci);
    }

    apply_relationships(&mut ak, &info.relationships);

    if !node.actions.is_empty() {
        for action in &node.actions {
            ak.add_action((*action).into());
        }
    }

    ak.set_bounds(bounds_to_rect(node.bounds));

    if !node.children.is_empty() {
        let child_ids: Vec<NodeId> = node
            .children
            .iter()
            .map(|c| element_id_to_node_id(c.id))
            .collect();
        ak.set_children(child_ids);
    }

    // `tab_index` has no direct accesskit field; surfaced via the Slate
    // contract for v1 and consumed during E20b platform wiring (post-v1).
    let _ = info.tab_index;

    ak
}

/// Flatten a node hierarchy into the `Vec<(NodeId, Node)>` an accesskit
/// `TreeUpdate` consumes. Depth-first; parent precedes its children.
pub fn to_accesskit_tree(node: &AccessibilityNode) -> Vec<(NodeId, Node)> {
    let mut out = Vec::new();
    push_node_recursive(node, &mut out);
    out
}

fn push_node_recursive(node: &AccessibilityNode, out: &mut Vec<(NodeId, Node)>) {
    out.push((element_id_to_node_id(node.id), to_accesskit_node(node)));
    for child in &node.children {
        push_node_recursive(child, out);
    }
}

/// Build a complete `TreeUpdate` from a window's top-level a11y nodes.
///
/// A platform `TreeUpdate` requires exactly one root, but the prepaint walk
/// can leave several top-level nodes (the view need not open a single root
/// a11y node). This synthesizes a `Role::Window` root owning them all, so the
/// shape is always valid. Always emits a *full* tree (no incremental diffing),
/// which keeps the macOS adapter and the lazy `request_initial_tree` in sync.
///
/// `focus` is the currently focused element, or the window root when nothing
/// is focused (`TreeUpdate::focus` is non-optional).
pub fn to_accesskit_tree_update(roots: &[AccessibilityNode], focus: Option<ElementId>) -> TreeUpdate {
    let mut nodes: Vec<(NodeId, Node)> = Vec::new();

    let mut window = Node::new(Role::Window);
    let child_ids: Vec<NodeId> = roots.iter().map(|n| element_id_to_node_id(n.id)).collect();
    window.set_children(child_ids);
    nodes.push((WINDOW_ROOT_NODE_ID, window));

    for root in roots {
        nodes.extend(to_accesskit_tree(root));
    }

    // `TreeUpdate::focus` must reference a node present in `nodes` — AccessKit
    // can assert on a dangling focus id. A focusable-but-a11y-suppressed element
    // (or one focused on a frame where its node isn't emitted) would dangle, so
    // fall back to the window root unless the focused id is actually in the tree.
    let focus = focus
        .map(element_id_to_node_id)
        .filter(|id| nodes.iter().any(|(nid, _)| nid == id))
        .unwrap_or(WINDOW_ROOT_NODE_ID);

    TreeUpdate {
        nodes,
        tree: Some(Tree::new(WINDOW_ROOT_NODE_ID)),
        tree_id: TreeId::ROOT,
        focus,
    }
}

fn apply_relationships(ak: &mut Node, rel: &AccessibilityRelationships) {
    if !rel.labelled_by.is_empty() {
        ak.set_labelled_by(translate_ids(&rel.labelled_by));
    }
    if !rel.described_by.is_empty() {
        ak.set_described_by(translate_ids(&rel.described_by));
    }
    if !rel.controls.is_empty() {
        ak.set_controls(translate_ids(&rel.controls));
    }
    if !rel.owns.is_empty() {
        ak.set_owns(translate_ids(&rel.owns));
    }
}

fn translate_ids(ids: &[ElementId]) -> Vec<NodeId> {
    ids.iter().copied().map(element_id_to_node_id).collect()
}

fn bounds_to_rect(b: Bounds) -> Rect {
    let x0 = b.origin.x as f64;
    let y0 = b.origin.y as f64;
    let x1 = x0 + b.size.width as f64;
    let y1 = y0 + b.size.height as f64;
    Rect { x0, y0, x1, y1 }
}
