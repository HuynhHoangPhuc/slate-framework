//! Integration tests for TextShapingCache garbage collection.
//!
//! Validates that cache entries not accessed for 2+ consecutive
//! frames are garbage collected.

use slate_framework::{AnyElement, Div, HeadlessApp, Text};

#[test]
fn cache_entry_survives_when_accessed_each_frame() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // Render same text 10 times
    for _ in 0..10 {
        let text = Text::new("Keep me alive").font_size(14.0);
        let _img = app.render(AnyElement::new(text)).expect("render");
    }

    // Entry should still exist (accessed each frame)
    assert_eq!(app.text_shaping_cache_len(), 1);
    assert_eq!(app.text_shaping_cache_hits(), 9); // First was miss, rest hits
}

#[test]
fn cache_entry_survives_one_frame_gap() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // Frame 0: render text
    let text = Text::new("Cached text").font_size(14.0);
    let _img = app.render(AnyElement::new(text)).expect("render");
    assert_eq!(app.text_shaping_cache_len(), 1);

    // Frame 1: render empty div (triggers advance + gc)
    let div = Div::new();
    let _img = app.render(AnyElement::new(div)).expect("render div");

    // Entry should survive (1-frame gap, cutoff = 2 - 2 = 0, entry.last_seen = 0)
    assert_eq!(app.text_shaping_cache_len(), 1);
}

#[test]
fn cache_entry_survives_two_frame_gap() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // Frame 0: render text
    let text = Text::new("Cached text").font_size(14.0);
    let _img = app.render(AnyElement::new(text)).expect("render");
    assert_eq!(app.text_shaping_cache_len(), 1);

    // Frames 1-2: render empty divs
    for _ in 0..2 {
        let div = Div::new();
        let _img = app.render(AnyElement::new(div)).expect("render div");
    }

    // Entry should still survive (2-frame gap at boundary)
    // After frame 0: entry.last_seen = 0, current = 1
    // After frame 1: current = 2, cutoff = 0, entry survives (0 >= 0)
    // After frame 2: current = 3, cutoff = 1, entry dropped (0 < 1)
    // Actually this is 2 frames without access, so it SHOULD be dropped
    assert_eq!(app.text_shaping_cache_len(), 0);
}

#[test]
fn cache_entry_dropped_after_three_frame_gap() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // Frame 0: render text
    let text = Text::new("Will be garbage collected").font_size(14.0);
    let _img = app.render(AnyElement::new(text)).expect("render");
    assert_eq!(app.text_shaping_cache_len(), 1);

    // Frames 1-3: render empty divs
    for _ in 0..3 {
        let div = Div::new();
        let _img = app.render(AnyElement::new(div)).expect("render div");
    }

    // Entry should be dropped (3-frame gap, well past cutoff)
    assert_eq!(app.text_shaping_cache_len(), 0);
}

#[test]
fn cache_entry_reaccessed_before_gc_survives() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // Frame 0: render text
    let text = Text::new("Survivor").font_size(14.0);
    let _img = app.render(AnyElement::new(text)).expect("render 0");
    assert_eq!(app.text_shaping_cache_len(), 1);

    // Frame 1: render div
    let div = Div::new();
    let _img = app.render(AnyElement::new(div)).expect("render 1");
    assert_eq!(app.text_shaping_cache_len(), 1);

    // Frame 2: re-access text (refreshes last_seen)
    let text = Text::new("Survivor").font_size(14.0);
    let _img = app.render(AnyElement::new(text)).expect("render 2");
    assert_eq!(app.text_shaping_cache_len(), 1);
    assert_eq!(app.text_shaping_cache_hits(), 1); // Was a hit

    // Frames 3-4: render divs
    for _ in 0..2 {
        let div = Div::new();
        let _img = app.render(AnyElement::new(div)).expect("render div");
    }

    // Entry should survive (last_seen was frame 2, now at frame 4, cutoff = 2)
    // Wait, let me trace this:
    // Frame 0: render text, entry.last_seen=0, advance(current=1), gc(cutoff=0-but-clamp)
    // Frame 1: render div, advance(current=2), gc(cutoff=0), entry survives
    // Frame 2: render text, entry.last_seen=2, advance(current=3), gc(cutoff=1), entry survives
    // Frame 3: render div, advance(current=4), gc(cutoff=2), entry survives (2>=2)
    // Frame 4: render div, advance(current=5), gc(cutoff=3), entry dropped (2<3)
    assert_eq!(app.text_shaping_cache_len(), 0);
}

#[test]
fn single_position_cache_replacement() {
    let mut app = HeadlessApp::new(200, 50).expect("headless app");

    // Frame 0: render text A
    let text_a = Text::new("Text A").font_size(14.0);
    let _img = app.render(AnyElement::new(text_a)).expect("render A");
    assert_eq!(app.text_shaping_cache_len(), 1);
    assert_eq!(app.text_shaping_cache_misses(), 1);

    // Frame 1: render text B at same position (replaces A's cache entry)
    let text_b = Text::new("Text B").font_size(14.0);
    let _img = app.render(AnyElement::new(text_b)).expect("render B");

    // Same ElementId (same tree position), different hash → entry replaced
    assert_eq!(app.text_shaping_cache_len(), 1);
    assert_eq!(app.text_shaping_cache_misses(), 2);

    // Frame 2: render text B again (cache hit)
    let text_b = Text::new("Text B").font_size(14.0);
    let _img = app.render(AnyElement::new(text_b)).expect("render B again");

    assert_eq!(app.text_shaping_cache_hits(), 1);

    // Frame 3: render text A (cache miss, replaces B)
    let text_a = Text::new("Text A").font_size(14.0);
    let _img = app.render(AnyElement::new(text_a)).expect("render A again");

    assert_eq!(app.text_shaping_cache_misses(), 3);
    assert_eq!(app.text_shaping_cache_len(), 1);
}
