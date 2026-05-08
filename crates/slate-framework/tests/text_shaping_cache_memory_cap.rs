//! Integration tests for TextShapingCache memory management.
//!
//! Phase 6 (D10): Validates memory tracking and per-element size limits.
//!
//! Note: LRU eviction at memory cap (50MB) is tested in paint_cache.rs unit tests
//! with a configurable small cap. Integration tests verify memory accounting works.

use slate_framework::{AnyElement, HeadlessApp, Text};

#[test]
fn memory_increases_with_cached_entries() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    let initial_memory = app.text_shaping_cache_memory();

    // Render text
    let text = Text::new("Hello, World!").font_size(14.0);
    let _img = app.render(AnyElement::new(text)).expect("render");

    // Memory should increase after caching
    let after_first = app.text_shaping_cache_memory();
    assert!(
        after_first > initial_memory,
        "memory should increase after caching: {} -> {}",
        initial_memory,
        after_first
    );
}

#[test]
fn memory_decreases_after_gc() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // Cache some text
    let text = Text::new("Will be garbage collected").font_size(14.0);
    let _img = app.render(AnyElement::new(text)).expect("render");

    let memory_after_cache = app.text_shaping_cache_memory();
    assert!(memory_after_cache > 0);

    // Trigger GC by rendering non-text elements for 3 frames
    for _ in 0..3 {
        let div = slate_framework::Div::new();
        let _img = app.render(AnyElement::new(div)).expect("render div");
    }

    // Memory should decrease after GC removes the entry
    let memory_after_gc = app.text_shaping_cache_memory();
    assert_eq!(
        memory_after_gc, 0,
        "memory should be 0 after entry GC'd: {}",
        memory_after_gc
    );
}

#[test]
fn memory_proportional_to_text_length() {
    let mut app = HeadlessApp::new(400, 50).expect("headless app");

    // Short text
    let short_text = Text::new("Hi").font_size(14.0);
    let _img = app.render(AnyElement::new(short_text)).expect("render short");
    let short_memory = app.text_shaping_cache_memory();

    // Clear by GC
    for _ in 0..3 {
        let div = slate_framework::Div::new();
        let _img = app.render(AnyElement::new(div)).expect("render div");
    }
    assert_eq!(app.text_shaping_cache_len(), 0);

    // Longer text
    let long_text = Text::new("This is a much longer piece of text that should use more memory").font_size(14.0);
    let _img = app.render(AnyElement::new(long_text)).expect("render long");
    let long_memory = app.text_shaping_cache_memory();

    // Longer text should use more memory (more glyphs)
    assert!(
        long_memory > short_memory,
        "longer text ({} chars) should use more memory than shorter ({} chars): {} vs {}",
        64, 2, long_memory, short_memory
    );
}

#[test]
fn cache_hit_does_not_increase_memory() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // First render (cache miss)
    let text = Text::new("Stable content").font_size(14.0);
    let _img = app.render(AnyElement::new(text)).expect("render 1");
    let memory_after_first = app.text_shaping_cache_memory();

    // Second render (cache hit)
    let text = Text::new("Stable content").font_size(14.0);
    let _img = app.render(AnyElement::new(text)).expect("render 2");
    let memory_after_second = app.text_shaping_cache_memory();

    // Memory should not increase on cache hit
    assert_eq!(
        memory_after_first, memory_after_second,
        "memory should not increase on cache hit: {} vs {}",
        memory_after_first, memory_after_second
    );
}

#[test]
fn memory_updates_on_content_change() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // First content
    let text1 = Text::new("Short").font_size(14.0);
    let _img = app.render(AnyElement::new(text1)).expect("render 1");
    let memory_first = app.text_shaping_cache_memory();

    // Different content at same position (replaces entry)
    let text2 = Text::new("Much longer content here").font_size(14.0);
    let _img = app.render(AnyElement::new(text2)).expect("render 2");
    let memory_second = app.text_shaping_cache_memory();

    // Memory should change (longer text)
    assert!(
        memory_second > memory_first,
        "memory should increase with longer replacement: {} vs {}",
        memory_first, memory_second
    );
}
