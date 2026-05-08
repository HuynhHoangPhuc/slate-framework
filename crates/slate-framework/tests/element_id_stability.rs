//! Integration tests for stable ElementId via tree-position keying (Phase 1 C3).

use slate_framework::types::ElementId;

#[test]
fn element_id_next_unique() {
    let id1 = ElementId::next();
    let id2 = ElementId::next();
    let id3 = ElementId::next();

    assert_ne!(id1, id2);
    assert_ne!(id2, id3);
    assert_ne!(id1, id3);
}

#[test]
fn element_id_from_raw_roundtrip() {
    let id = ElementId::from_raw(12345);
    assert_eq!(id.as_u64(), 12345);
}

#[test]
fn element_id_equality() {
    let id1 = ElementId::from_raw(100);
    let id2 = ElementId::from_raw(100);
    let id3 = ElementId::from_raw(200);

    assert_eq!(id1, id2);
    assert_ne!(id1, id3);
}

#[test]
fn element_id_hash() {
    use std::collections::HashSet;

    let mut set = HashSet::new();
    set.insert(ElementId::from_raw(1));
    set.insert(ElementId::from_raw(2));
    set.insert(ElementId::from_raw(1)); // duplicate

    assert_eq!(set.len(), 2);
}

#[test]
fn element_id_debug() {
    let id = ElementId::from_raw(42);
    let debug = format!("{:?}", id);
    assert!(debug.contains("42"));
}

// Tree-position keying stability tests require HeadlessApp integration.
// The core hashing logic is validated in context::tests::allocate_id_stability_across_calls.
// These tests verify the public API contract.

#[test]
fn div_key_builder() {
    use slate_framework::Div;

    let div = Div::new().key("toolbar");
    // key() consumed during prepaint - we just verify the builder works
    drop(div);
}

#[test]
fn text_key_builder() {
    use slate_framework::Text;

    let text = Text::new("item").key("list-item-1");
    drop(text);
}

#[test]
fn div_with_keyed_children() {
    use slate_framework::{Div, Text};

    let div = Div::new()
        .child(Text::new("first").key("item-a"))
        .child(Text::new("second").key("item-b"))
        .child(Text::new("third").key("item-c"));

    // Keyed children should produce stable IDs across frames
    // Full validation requires HeadlessApp multi-frame test
    drop(div);
}
