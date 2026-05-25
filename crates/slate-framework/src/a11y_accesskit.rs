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

use accesskit::{Node, NodeId, Rect};

use crate::types::{
    AccessibilityNode, AccessibilityRelationships, Bounds, ElementId, LiveRegion,
};

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
