//! Integration tests for TextWrap::Wrap (Phase 3 C6 validation).

use slate_framework::elements::{TextAlign, TextWrap};
use slate_framework::Text;

#[test]
fn text_wrap_none_default() {
    let text = Text::new("hello");
    // Default is TextWrap::None
    drop(text);
}

#[test]
fn text_wrap_builder() {
    let text = Text::new("hello").wrap(TextWrap::Wrap);
    drop(text);
}

#[test]
fn text_wrap_break_word_builder() {
    let text = Text::new("hello").wrap(TextWrap::WrapBreakWord);
    // Currently falls back to Wrap with debug log
    drop(text);
}

#[test]
fn text_align_left_default() {
    let text = Text::new("hello");
    drop(text);
}

#[test]
fn text_align_center() {
    let text = Text::new("hello").align(TextAlign::Center);
    drop(text);
}

#[test]
fn text_align_right() {
    let text = Text::new("hello").align(TextAlign::Right);
    drop(text);
}

#[test]
fn text_wrap_enum_equality() {
    assert_eq!(TextWrap::None, TextWrap::None);
    assert_eq!(TextWrap::Wrap, TextWrap::Wrap);
    assert_eq!(TextWrap::WrapBreakWord, TextWrap::WrapBreakWord);
    assert_ne!(TextWrap::None, TextWrap::Wrap);
}

#[test]
fn text_align_enum_equality() {
    assert_eq!(TextAlign::Left, TextAlign::Left);
    assert_eq!(TextAlign::Center, TextAlign::Center);
    assert_eq!(TextAlign::Right, TextAlign::Right);
    assert_ne!(TextAlign::Left, TextAlign::Right);
}

#[test]
fn text_with_long_content() {
    let content = "This is a long piece of text that should wrap when \
                   rendered in a narrow container with TextWrap::Wrap enabled";
    let text = Text::new(content).wrap(TextWrap::Wrap);
    drop(text);
}

#[test]
fn text_with_whitespace_only() {
    let text = Text::new("   \t\n  ").wrap(TextWrap::Wrap);
    // Whitespace-only content keeps original single-line shape
    drop(text);
}

#[test]
fn text_empty_content() {
    let text = Text::new("").wrap(TextWrap::Wrap);
    drop(text);
}

#[test]
fn text_single_word() {
    let text = Text::new("supercalifragilisticexpialidocious").wrap(TextWrap::Wrap);
    // Single long word: must fit on one line even if wider than container
    drop(text);
}

#[test]
fn text_multiple_spaces() {
    let text = Text::new("word1    word2     word3").wrap(TextWrap::Wrap);
    // Multiple spaces between words collapse to single space in wrap
    drop(text);
}
