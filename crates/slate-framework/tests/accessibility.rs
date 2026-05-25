//! Integration tests for hierarchical accessibility tree.

mod common;

use slate_framework::types::{AccessibilityNode, AccessibilityRole};
use slate_framework::{Div, Text};

/// Render an element tree headlessly and return the completed a11y tree.
fn render_and_get_a11y(element: impl slate_framework::IntoElement) -> Vec<AccessibilityNode> {
    let mut app = slate_framework::HeadlessApp::new(800, 600).expect("HeadlessApp creation");
    let root = slate_framework::AnyElement::new(element);
    app.render(root).expect("headless render");
    app.a11y_nodes()
}

#[test]
fn div_has_group_role() {
    let div = Div::new();
    let info = slate_framework::Element::accessibility(&div);
    assert!(info.is_some());
    assert_eq!(info.unwrap().role, AccessibilityRole::Group);
}

#[test]
fn text_has_label_role() {
    let text = Text::new("Hello");
    let info = slate_framework::Element::accessibility(&text);
    assert!(info.is_some());
    let info = info.unwrap();
    assert_eq!(info.role, AccessibilityRole::Label);
    assert_eq!(info.label, Some("Hello".to_string()));
}

#[test]
fn text_label_matches_content() {
    let text = Text::new("World");
    let info = slate_framework::Element::accessibility(&text).unwrap();
    assert_eq!(info.label, Some("World".to_string()));
}

#[test]
fn accessibility_role_to_accesskit_mapping() {
    use accesskit::Role as AkRole;

    assert_eq!(AkRole::from(AccessibilityRole::Button), AkRole::Button);
    assert_eq!(AkRole::from(AccessibilityRole::Group), AkRole::Group);
    assert_eq!(AkRole::from(AccessibilityRole::Label), AkRole::Label);
    assert_eq!(
        AkRole::from(AccessibilityRole::TextInput),
        AkRole::TextInput
    );
    assert_eq!(AkRole::from(AccessibilityRole::Checkbox), AkRole::CheckBox);
    assert_eq!(AkRole::from(AccessibilityRole::Slider), AkRole::Slider);
}

#[test]
fn accessibility_info_default() {
    let info = slate_framework::types::AccessibilityInfo::default();
    assert_eq!(info.role, AccessibilityRole::Unknown);
    assert!(info.label.is_none());
    assert!(!info.is_disabled);
    assert!(!info.is_focused);
    assert!(info.relationships.labelled_by.is_empty());
    assert!(info.relationships.described_by.is_empty());
    assert!(info.relationships.controls.is_empty());
    assert!(info.relationships.owns.is_empty());
    assert!(info.tab_index.is_none());
}

#[test]
fn a11y_tree_div_with_two_texts() {
    let tree = render_and_get_a11y(
        Div::new()
            .child(Text::new("Hello"))
            .child(Text::new("World")),
    );

    assert_eq!(tree.len(), 1, "expected a single root Group node");
    let root = &tree[0];
    assert_eq!(root.info.role, AccessibilityRole::Group);
    assert_eq!(root.children.len(), 2, "Group should hold two Label children");
    assert_eq!(root.children[0].info.role, AccessibilityRole::Label);
    assert_eq!(root.children[0].info.label.as_deref(), Some("Hello"));
    assert_eq!(root.children[1].info.role, AccessibilityRole::Label);
    assert_eq!(root.children[1].info.label.as_deref(), Some("World"));
}
