//! Integration tests for hierarchical accessibility tree (Phase 2 validation).

mod common;

use slate_framework::types::{AccessibilityNode, AccessibilityRole};
use slate_framework::{Div, Text};

/// Helper to extract a11y nodes from an element tree via HeadlessApp.
#[allow(dead_code)]
fn render_and_get_a11y(element: impl slate_framework::IntoElement) -> Vec<AccessibilityNode> {
    let app = slate_framework::HeadlessApp::new(800, 600).expect("HeadlessApp creation");
    let any_element = slate_framework::AnyElement::new(element);

    // HeadlessApp doesn't expose a11y nodes directly, so we test via the
    // element tree structure. For full a11y testing, we'd need to expose
    // the a11y_completed vec from HeadlessApp or add a dedicated API.
    // For now, we validate the tree structure via Element::accessibility().
    drop(app);
    drop(any_element);

    // Return empty for now - actual a11y tree testing requires HeadlessApp API changes
    Vec::new()
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
}
