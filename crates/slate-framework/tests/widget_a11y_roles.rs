//! Per-widget a11y role / label / value verification.
//!
//! Renders each interactive widget headlessly and asserts its emitted a11y node
//! announces the right role, name, and value. Complements the conversion-
//! contract tests in `accessibility_accesskit_conversion.rs` (which hand-build
//! nodes): these prove the *widgets themselves* populate the a11y tree, so a
//! regression that drops a role/value fails CI.

mod common;

use common::dump_a11y_tree;
use slate_framework::types::{
    AccessibilityAction, AccessibilityNode, AccessibilityRole, Bounds,
};
use slate_framework::{AnyElement, Column, DataGrid, HeadlessApp, Select, TextArea, TextField};
use slate_reactive::Signal;

/// Depth-first search for the first node with `role`.
fn find_role(nodes: &[AccessibilityNode], role: AccessibilityRole) -> Option<&AccessibilityNode> {
    for n in nodes {
        if n.info.role == role {
            return Some(n);
        }
        if let Some(found) = find_role(&n.children, role) {
            return Some(found);
        }
    }
    None
}

/// Collect every node carrying `role` (depth-first).
fn collect_role<'a>(
    nodes: &'a [AccessibilityNode],
    role: AccessibilityRole,
    out: &mut Vec<&'a AccessibilityNode>,
) {
    for n in nodes {
        if n.info.role == role {
            out.push(n);
        }
        collect_role(&n.children, role, out);
    }
}

#[test]
fn text_field_announces_text_input_with_value_and_label() {
    let mut app = HeadlessApp::new(800, 600).expect("headless app");
    let value = Signal::new(app.runtime(), "alice@example.com".to_string());
    let field = TextField::new(value).a11y_label("Email");
    app.render(AnyElement::new(field)).expect("render");

    let nodes = app.a11y_nodes();
    let n = find_role(&nodes, AccessibilityRole::TextInput)
        .unwrap_or_else(|| panic!("no TextInput node:\n{}", dump_a11y_tree(&nodes)));
    assert_eq!(n.info.label.as_deref(), Some("Email"));
    assert_eq!(n.info.value.as_deref(), Some("alice@example.com"));
    assert!(
        n.actions.contains(&AccessibilityAction::Focus),
        "text input must expose the Focus action so a screen reader can reach it"
    );
}

#[test]
fn text_area_announces_text_input_with_value() {
    let mut app = HeadlessApp::new(800, 600).expect("headless app");
    let value = Signal::new(app.runtime(), "line one\nline two".to_string());
    let area = TextArea::new(value).a11y_label("Notes");
    app.render(AnyElement::new(area)).expect("render");

    let nodes = app.a11y_nodes();
    let n = find_role(&nodes, AccessibilityRole::TextInput)
        .unwrap_or_else(|| panic!("no TextInput node:\n{}", dump_a11y_tree(&nodes)));
    assert_eq!(n.info.label.as_deref(), Some("Notes"));
    assert_eq!(n.info.value.as_deref(), Some("line one\nline two"));
}

#[test]
fn select_trigger_announces_button_role_with_current_value() {
    let mut app = HeadlessApp::new(800, 600).expect("headless app");
    let selected = Signal::new(app.runtime(), 1usize);
    let open = Signal::new(app.runtime(), false);
    let anchor = Signal::new(app.runtime(), Bounds::from_origin_size(0.0, 0.0, 0.0, 0.0));
    let select = Select::new(selected, open, anchor).options(["Low", "Medium", "High"]);
    app.render(AnyElement::new(select)).expect("render");

    let nodes = app.a11y_nodes();
    // The closed trigger announces as an actionable control named by the
    // current selection (index 1 = "Medium").
    let n = find_role(&nodes, AccessibilityRole::Button)
        .unwrap_or_else(|| panic!("no Button (Select trigger) node:\n{}", dump_a11y_tree(&nodes)));
    assert_eq!(n.info.label.as_deref(), Some("Medium"));
}

#[test]
fn data_grid_registers_all_logical_cells_with_truthful_row_count() {
    // A short viewport that cannot fit all rows forces virtualization; the a11y
    // tree must still carry every logical row and a truthful total row_count
    // (the locked S0 invariant: register all cells, window only the glyphs).
    let mut app = HeadlessApp::new(400, 120).expect("headless app");
    let active = Signal::new(app.runtime(), (0usize, 0usize));
    let offset = Signal::new(app.runtime(), 0.0f32);
    let columns = vec![Column::new("Name", 120.0), Column::new("PID", 80.0)];
    let rows: Vec<Vec<String>> = (0..20)
        .map(|i| vec![format!("proc-{i}"), format!("{}", 1000 + i)])
        .collect();
    let grid = DataGrid::new(columns, rows, active, offset);
    app.render(AnyElement::new(grid)).expect("render");

    let nodes = app.a11y_nodes();
    let grid_node = find_role(&nodes, AccessibilityRole::Grid)
        .unwrap_or_else(|| panic!("no Grid node:\n{}", dump_a11y_tree(&nodes)));
    // Truthful total — 20 data rows + 1 sticky header row — not the windowed
    // visible count (ARIA row_count includes the header row).
    assert_eq!(
        grid_node.info.row_count,
        Some(21),
        "grid must report the full logical row count (incl. header), not the windowed count"
    );

    // Every logical data cell registered (20 rows x 2 cols), regardless of the
    // visible window — `set_focus` drops unregistered ids, so Page/Home/End
    // jumps onto off-window cells only work because all cells are present.
    let mut cells = Vec::new();
    collect_role(&nodes, AccessibilityRole::Cell, &mut cells);
    assert_eq!(
        cells.len(),
        40,
        "expected all 20x2 logical cells registered, got {}:\n{}",
        cells.len(),
        dump_a11y_tree(&nodes)
    );
}
