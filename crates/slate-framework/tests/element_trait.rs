//! Integration tests for Element trait lifecycle and type erasure.

use slate_framework::{AnyElement, Div, Element, IntoElement, Text};

#[test]
fn div_into_element() {
    let div = Div::new();
    let element = div.into_element();
    assert!(element.accessibility().is_some());
}

#[test]
fn text_into_element() {
    let text = Text::new("test");
    let element = text.into_element();
    let info = element.accessibility().unwrap();
    assert_eq!(info.label, Some("test".to_string()));
}

#[test]
fn any_element_wraps_div() {
    let div = Div::new().background([1.0, 0.0, 0.0, 1.0]);
    let any = AnyElement::new(div);
    // AnyElement is type-erased; we can't access inner directly
    // but we can verify it was created
    drop(any);
}

#[test]
fn any_element_wraps_text() {
    let text = Text::new("hello");
    let any = AnyElement::new(text);
    drop(any);
}

#[test]
fn div_child_builder() {
    let div = Div::new()
        .child(Text::new("one"))
        .child(Text::new("two"))
        .child(Text::new("three"));
    // Div stores children internally
    drop(div);
}

#[test]
fn div_children_iterator() {
    let texts = vec!["a", "b", "c"];
    let div = Div::new().children(texts.into_iter().map(Text::new));
    drop(div);
}

#[test]
fn div_child_any() {
    let inner = AnyElement::new(Text::new("inner"));
    let div = Div::new().child_any(inner);
    drop(div);
}

#[test]
fn element_id_default_none() {
    let div = Div::new();
    // Before prepaint, id() returns None
    assert!(div.id().is_none());
}

#[test]
fn text_element_id_default_none() {
    let text = Text::new("test");
    assert!(text.id().is_none());
}

#[test]
fn string_into_element() {
    let s = String::from("hello");
    let text: Text = s.into_element();
    let info = text.accessibility().unwrap();
    assert_eq!(info.label, Some("hello".to_string()));
}

#[test]
fn static_str_into_element() {
    let text: Text = "static".into_element();
    let info = text.accessibility().unwrap();
    assert_eq!(info.label, Some("static".to_string()));
}
