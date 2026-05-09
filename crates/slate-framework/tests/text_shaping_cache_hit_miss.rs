//! Integration tests for TextShapingCache hit/miss behavior.
//!
//! Phase 6 (D10): Validates that the text shaping cache correctly:
//! - Misses on first render (shape closure invoked)
//! - Hits on second render with unchanged content/style/bounds (shape closure NOT invoked)
//! - Misses after content change (shape closure invoked)

use slate_framework::{AnyElement, HeadlessApp, Text};

#[test]
fn cache_misses_on_first_render() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    let text = Text::new("Hello, World!").font_size(14.0);
    let _img = app.render(AnyElement::new(text)).expect("render");

    // First render should be a miss
    assert_eq!(app.text_shaping_cache_misses(), 1);
    assert_eq!(app.text_shaping_cache_hits(), 0);
}

#[test]
fn cache_hits_on_second_render_same_content() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // First render: miss
    let text1 = Text::new("Hello, World!").font_size(14.0);
    let _img1 = app.render(AnyElement::new(text1)).expect("first render");

    let misses_after_first = app.text_shaping_cache_misses();
    let hits_after_first = app.text_shaping_cache_hits();

    // Second render with same content: hit
    let text2 = Text::new("Hello, World!").font_size(14.0);
    let _img2 = app.render(AnyElement::new(text2)).expect("second render");

    // Should have one more hit, no new misses
    assert_eq!(app.text_shaping_cache_hits(), hits_after_first + 1);
    assert_eq!(app.text_shaping_cache_misses(), misses_after_first);
}

#[test]
fn cache_misses_after_content_change() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // First render
    let text1 = Text::new("Hello, World!").font_size(14.0);
    let _img1 = app.render(AnyElement::new(text1)).expect("first render");

    let misses_after_first = app.text_shaping_cache_misses();

    // Second render with different content: miss
    let text2 = Text::new("Goodbye, World!").font_size(14.0);
    let _img2 = app.render(AnyElement::new(text2)).expect("second render");

    // Should have one more miss (different content)
    assert_eq!(app.text_shaping_cache_misses(), misses_after_first + 1);
}

#[test]
fn cache_misses_after_style_change() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // First render with font size 14
    let text1 = Text::new("Hello").font_size(14.0);
    let _img1 = app.render(AnyElement::new(text1)).expect("first render");

    let misses_after_first = app.text_shaping_cache_misses();
    let hits_after_first = app.text_shaping_cache_hits();

    // Second render with different font size: miss
    let text2 = Text::new("Hello").font_size(20.0);
    let _img2 = app.render(AnyElement::new(text2)).expect("second render");

    // Different font size → different hash → miss
    assert_eq!(app.text_shaping_cache_misses(), misses_after_first + 1);
    assert_eq!(app.text_shaping_cache_hits(), hits_after_first);
}

#[test]
fn cache_entry_survives_across_frames() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // Render 5 times with same content
    for i in 0..5 {
        let text = Text::new("Stable content").font_size(14.0);
        let _img = app
            .render(AnyElement::new(text))
            .unwrap_or_else(|_| panic!("render {}", i));
    }

    // First was a miss, rest were hits
    assert_eq!(app.text_shaping_cache_misses(), 1);
    assert_eq!(app.text_shaping_cache_hits(), 4);
    assert_eq!(app.text_shaping_cache_len(), 1);
}

#[test]
fn same_tree_position_replaces_cache_entry() {
    let mut app = HeadlessApp::new(300, 100).expect("headless app");

    // First render with one text element
    let text1 = Text::new("First text").font_size(14.0);
    let _img1 = app.render(AnyElement::new(text1)).expect("first render");

    // Second render with different content at same tree position
    let text2 = Text::new("Second text").font_size(14.0);
    let _img2 = app.render(AnyElement::new(text2)).expect("second render");

    // Two misses (different content → different hash → replaces entry)
    assert_eq!(app.text_shaping_cache_misses(), 2);
    // Only one entry because same ElementId (same tree position)
    assert_eq!(app.text_shaping_cache_len(), 1);

    // Re-render second text (current cache content): hit
    let text2_again = Text::new("Second text").font_size(14.0);
    let _img3 = app
        .render(AnyElement::new(text2_again))
        .expect("third render");

    assert_eq!(app.text_shaping_cache_hits(), 1);

    // Re-render first text: miss (cache was overwritten)
    let text1_again = Text::new("First text").font_size(14.0);
    let _img4 = app
        .render(AnyElement::new(text1_again))
        .expect("fourth render");

    assert_eq!(app.text_shaping_cache_misses(), 3);
}
