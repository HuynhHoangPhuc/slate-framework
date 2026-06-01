//! Regression: widgets that build child label elements and paint them
//! (List, Tree, VirtualList, DataGrid) MUST prepaint those children first.
//!
//! Each widget here is rendered through the full `HeadlessApp` layout → prepaint
//! → paint pipeline. Before the fix, a widget that called `child.paint()` without
//! a matching `child.prepaint()` aborted with `"paint called before prepaint"`
//! (the `ElementState` wrapper requires the prepaint-produced paint state). These
//! tests fail (panic) if any data-display widget regresses to that pattern.

use std::collections::HashSet;

use slate_framework::reactive::Signal;
use slate_framework::{AnyElement, Column, DataGrid, HeadlessApp, List, Tree, TreeNode, VirtualList};

#[test]
fn list_renders_without_prepaint_panic() {
    let mut app = HeadlessApp::new(300, 200).expect("headless app");
    let selected = Signal::new(app.runtime(), Some(0usize));
    let list = List::new(["CPU", "Memory", "Disk"], selected).label("Resources");
    assert!(app.render(AnyElement::new(list)).is_ok());
}

#[test]
fn tree_renders_without_prepaint_panic() {
    let mut app = HeadlessApp::new(300, 200).expect("headless app");
    let rt = app.runtime();
    let expanded = Signal::new(rt.clone(), HashSet::from([0u64]));
    let selected = Signal::new(rt, None);
    let nodes = vec![TreeNode::new(0, "System").children([TreeNode::new(1, "kernel_task")])];
    let tree = Tree::new(nodes, expanded, selected).label("Processes");
    assert!(app.render(AnyElement::new(tree)).is_ok());
}

#[test]
fn virtual_list_renders_without_prepaint_panic() {
    let mut app = HeadlessApp::new(300, 200).expect("headless app");
    let rt = app.runtime();
    let selected = Signal::new(rt.clone(), None);
    let offset = Signal::new(rt, 0.0);
    let vl = VirtualList::new((0..50).map(|i| format!("row {i}")), selected, offset).height(180.0);
    assert!(app.render(AnyElement::new(vl)).is_ok());
}

#[test]
fn data_grid_renders_without_prepaint_panic() {
    let mut app = HeadlessApp::new(420, 240).expect("headless app");
    let rt = app.runtime();
    let active = Signal::new(rt.clone(), (0usize, 0usize));
    let offset = Signal::new(rt, 0.0);
    let cols = vec![Column::new("PID", 80.0), Column::new("Process", 200.0)];
    let rows: Vec<Vec<String>> = (0..40).map(|i| vec![format!("{}", 100 + i), format!("proc-{i}")]).collect();
    let grid = DataGrid::new(cols, rows, active, offset).height(200.0);
    assert!(app.render(AnyElement::new(grid)).is_ok());
}
