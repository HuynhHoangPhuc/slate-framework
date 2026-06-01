//! Tree-shape contract for the DataGrid a11y spike: grid/row/cell roles,
//! zero-based cell indices, and the synthesized-window-root `TreeUpdate`.
//!
//! Hand-built `AccessibilityNode` trees (no rendering) keep this pinned to the
//! conversion contract the macOS adapter relies on.

use accesskit::{NodeId, Role};
use slate_framework::a11y_accesskit::{
    WINDOW_ROOT_NODE_ID, element_id_to_node_id, to_accesskit_node, to_accesskit_tree_update,
};
use slate_framework::types::{
    AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, ElementId,
};

fn node(id: u64, info: AccessibilityInfo, children: Vec<AccessibilityNode>) -> AccessibilityNode {
    AccessibilityNode {
        id: ElementId::from_raw(id),
        bounds: Bounds::from_origin_size(0.0, 0.0, 100.0, 30.0),
        info,
        children,
        actions: Vec::new(),
    }
}

fn cell(id: u64, role: AccessibilityRole, row: usize, col: usize) -> AccessibilityNode {
    node(
        id,
        AccessibilityInfo {
            role,
            label: Some(format!("r{row}c{col}")),
            row_index: Some(row),
            column_index: Some(col),
            ..Default::default()
        },
        Vec::new(),
    )
}

#[test]
fn grid_roles_map_to_accesskit() {
    assert_eq!(Role::from(AccessibilityRole::Grid), Role::Grid);
    assert_eq!(Role::from(AccessibilityRole::Row), Role::Row);
    // `Cell` is the interactive grid variant.
    assert_eq!(Role::from(AccessibilityRole::Cell), Role::GridCell);
    assert_eq!(Role::from(AccessibilityRole::ColumnHeader), Role::ColumnHeader);
    assert_eq!(Role::from(AccessibilityRole::RowHeader), Role::RowHeader);
}

#[test]
fn cell_indices_round_trip() {
    let ak = to_accesskit_node(&cell(5, AccessibilityRole::Cell, 2, 1));
    assert_eq!(ak.role(), Role::GridCell);
    assert_eq!(ak.row_index(), Some(2));
    assert_eq!(ak.column_index(), Some(1));
}

#[test]
fn grid_container_carries_counts() {
    let grid = node(
        1,
        AccessibilityInfo {
            role: AccessibilityRole::Grid,
            row_count: Some(5),
            column_count: Some(3),
            ..Default::default()
        },
        Vec::new(),
    );
    let ak = to_accesskit_node(&grid);
    assert_eq!(ak.role(), Role::Grid);
    assert_eq!(ak.row_count(), Some(5));
    assert_eq!(ak.column_count(), Some(3));
}

/// A 1-row × 2-col grid: Grid > Row > {ColumnHeader, Cell}.
fn sample_grid() -> AccessibilityNode {
    let row = node(
        10,
        AccessibilityInfo {
            role: AccessibilityRole::Row,
            row_index: Some(0),
            ..Default::default()
        },
        vec![
            cell(100, AccessibilityRole::ColumnHeader, 0, 0),
            cell(101, AccessibilityRole::Cell, 0, 1),
        ],
    );
    node(
        1,
        AccessibilityInfo {
            role: AccessibilityRole::Grid,
            row_count: Some(1),
            column_count: Some(2),
            ..Default::default()
        },
        vec![row],
    )
}

#[test]
fn tree_update_synthesizes_window_root_and_tracks_focus() {
    let focus_id = ElementId::from_raw(101);
    let update = to_accesskit_tree_update(&[sample_grid()], Some(focus_id));

    // Window root is first, with the grid as its child.
    assert_eq!(update.nodes[0].0, WINDOW_ROOT_NODE_ID);
    assert_eq!(update.nodes[0].1.role(), Role::Window);
    assert_eq!(update.nodes[0].1.children(), &[NodeId(1)]);

    // Tree root + all 4 grid nodes flattened beneath (window + grid + row + 2 cells = 5).
    assert_eq!(update.tree.as_ref().unwrap().root, WINDOW_ROOT_NODE_ID);
    assert_eq!(update.nodes.len(), 5);

    // Focus carried to the requested cell.
    assert_eq!(update.focus, element_id_to_node_id(focus_id));
}

#[test]
fn tree_update_defaults_focus_to_window_root() {
    let update = to_accesskit_tree_update(&[sample_grid()], None);
    assert_eq!(update.focus, WINDOW_ROOT_NODE_ID);
}
