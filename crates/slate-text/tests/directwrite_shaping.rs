//! DirectWrite shaping tests (Windows only).

#![cfg(target_os = "windows")]

use slate_text::{DirectWriteBackend, TextBackend, TEST_FONT};

#[test]
fn shapes_hello_world() {
    let mut backend = DirectWriteBackend::new().expect("backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("font load");

    let shaped = backend.shape_line(&font, "Hello").expect("shape");

    assert_eq!(shaped.glyphs.len(), 5, "expected 5 glyphs for 'Hello'");
    assert!(
        shaped.glyphs.iter().all(|g| g.glyph_id != 0),
        "all glyphs should have non-zero IDs"
    );
    assert!(shaped.width_lpx > 0.0, "shaped line should have width");

    // Check positions are monotonically increasing (LTR)
    let mut x = 0.0f32;
    for g in &shaped.glyphs {
        assert!(g.x_advance_lpx > 0.0, "each glyph should have positive advance");
        x += g.x_advance_lpx;
    }
    assert!((x - shaped.width_lpx).abs() < 0.001, "width should equal sum of advances");
}

#[test]
fn empty_string_returns_empty_line() {
    let mut backend = DirectWriteBackend::new().expect("backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("font load");

    let shaped = backend.shape_line(&font, "").expect("shape empty");

    assert!(shaped.glyphs.is_empty(), "empty string should produce no glyphs");
    assert_eq!(shaped.width_lpx, 0.0, "empty line should have zero width");
}

#[test]
fn zwsp_does_not_panic() {
    let mut backend = DirectWriteBackend::new().expect("backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("font load");

    // Zero-width space - Zed #10749 mitigation
    let result = backend.shape_line(&font, "\u{200B}");
    assert!(result.is_ok(), "ZWSP should not panic");
}

#[test]
fn shapes_ascii_printable() {
    let mut backend = DirectWriteBackend::new().expect("backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 14.0, 1.0)
        .expect("font load");

    let text = "The quick brown fox jumps over the lazy dog. 0123456789!@#$%";
    let shaped = backend.shape_line(&font, text).expect("shape");

    assert_eq!(shaped.glyphs.len(), text.chars().count());
    assert!(shaped.width_lpx > 0.0);
}
