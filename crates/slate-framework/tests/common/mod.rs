//! Common test helpers for slate-framework integration tests.

#![allow(dead_code)]

use slate_framework::types::AccessibilityNode;

/// Pretty-print an accessibility tree for assertion error messages.
pub fn dump_a11y_tree(nodes: &[AccessibilityNode]) -> String {
    fn dump_node(node: &AccessibilityNode, indent: usize, out: &mut String) {
        let prefix = "  ".repeat(indent);
        out.push_str(&format!(
            "{}- id={} role={:?} label={:?}\n",
            prefix,
            node.id.as_u64(),
            node.info.role,
            node.info.label
        ));
        for child in &node.children {
            dump_node(child, indent + 1, out);
        }
    }

    let mut out = String::new();
    for node in nodes {
        dump_node(node, 0, &mut out);
    }
    out
}

/// Count total nodes in an accessibility tree (including nested children).
pub fn count_a11y_nodes(nodes: &[AccessibilityNode]) -> usize {
    fn count_recursive(node: &AccessibilityNode) -> usize {
        1 + node.children.iter().map(count_recursive).sum::<usize>()
    }
    nodes.iter().map(count_recursive).sum()
}
