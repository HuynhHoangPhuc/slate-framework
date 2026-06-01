//! [`TreeNode`] — the caller's hierarchical model — and the visible-node
//! flattening that turns it into the 1-D focus ring [`Tree`](super::Tree) navigates.
//!
//! Expansion is caller-owned: a node is expanded iff its id is in the
//! `expanded` set, and a node's children are visible only when every ancestor
//! (including itself) is expanded. Flattening is depth-first, so the visible
//! order matches what the user sees top-to-bottom.

use std::collections::HashSet;

/// One node of a [`Tree`](super::Tree)'s caller-owned model: a stable id, a
/// label, and zero or more children. Build a forest of these and pass the roots
/// to [`Tree::new`](super::Tree::new).
#[derive(Clone)]
pub struct TreeNode {
    /// Stable, caller-assigned id — used as the expansion/selection key and to
    /// derive a stable element id, so it must be unique within the tree.
    pub id: u64,
    /// Visible text + accessible name of the node.
    pub label: String,
    /// Child nodes (empty for a leaf).
    pub children: Vec<TreeNode>,
}

impl TreeNode {
    /// Create a leaf node with the given stable id and label.
    pub fn new(id: u64, label: impl Into<String>) -> Self {
        Self { id, label: label.into(), children: Vec::new() }
    }

    /// Append a child node (builder style).
    pub fn child(mut self, child: TreeNode) -> Self {
        self.children.push(child);
        self
    }

    /// Append several child nodes (builder style).
    pub fn children<I: IntoIterator<Item = TreeNode>>(mut self, children: I) -> Self {
        self.children.extend(children);
        self
    }
}

/// One row of the flattened, currently-visible tree — what a single focus-ring
/// position renders and announces.
pub(super) struct VisibleNode {
    pub id: u64,
    pub label: String,
    /// Indentation depth (0 = a root).
    pub depth: usize,
    /// Whether the node has any children (drives the twisty + Expand/Collapse).
    pub has_children: bool,
    /// Whether the node is currently expanded.
    pub expanded: bool,
    /// Whether the node is the committed selection.
    pub selected: bool,
}

/// Flatten the visible nodes of `roots` depth-first: a node is always listed,
/// and its children are listed (recursively) only when it is expanded.
pub(super) fn flatten_visible(
    roots: &[TreeNode],
    expanded: &HashSet<u64>,
    selected: Option<u64>,
) -> Vec<VisibleNode> {
    let mut out = Vec::new();
    push_visible(roots, 0, expanded, selected, &mut out);
    out
}

fn push_visible(
    nodes: &[TreeNode],
    depth: usize,
    expanded: &HashSet<u64>,
    selected: Option<u64>,
    out: &mut Vec<VisibleNode>,
) {
    for node in nodes {
        let is_expanded = expanded.contains(&node.id);
        let has_children = !node.children.is_empty();
        out.push(VisibleNode {
            id: node.id,
            label: node.label.clone(),
            depth,
            has_children,
            expanded: is_expanded,
            selected: selected == Some(node.id),
        });
        if has_children && is_expanded {
            push_visible(&node.children, depth + 1, expanded, selected, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<TreeNode> {
        vec![
            TreeNode::new(1, "root-a").children([
                TreeNode::new(2, "a-1"),
                TreeNode::new(3, "a-2").child(TreeNode::new(4, "a-2-i")),
            ]),
            TreeNode::new(5, "root-b"),
        ]
    }

    #[test]
    fn collapsed_roots_only_show_roots() {
        let expanded = HashSet::new();
        let v = flatten_visible(&fixture(), &expanded, None);
        assert_eq!(v.iter().map(|n| n.id).collect::<Vec<_>>(), vec![1, 5]);
        assert!(v[0].has_children && !v[0].expanded);
        assert!(!v[1].has_children);
    }

    #[test]
    fn expanding_a_node_reveals_its_children_with_depth() {
        let expanded = HashSet::from([1]);
        let v = flatten_visible(&fixture(), &expanded, Some(3));
        assert_eq!(v.iter().map(|n| n.id).collect::<Vec<_>>(), vec![1, 2, 3, 5]);
        assert_eq!(v[1].depth, 1, "a-1 is one level deep");
        assert!(v[2].selected, "a-2 is the selected node");
        // a-2 (id 3) is collapsed, so its child id 4 is not visible.
        assert!(!v.iter().any(|n| n.id == 4));
    }

    #[test]
    fn deep_expansion_reveals_grandchildren() {
        let expanded = HashSet::from([1, 3]);
        let v = flatten_visible(&fixture(), &expanded, None);
        assert_eq!(v.iter().map(|n| n.id).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
        assert_eq!(v[3].depth, 2, "a-2-i is two levels deep");
    }
}
