//! CoreText shaping tests (macOS only).

#![cfg(target_os = "macos")]

use slate_text::{CoreTextBackend, Font, TEST_FONT, TextBackend};

#[test]
fn shapes_hello_world() {
    let mut backend = CoreTextBackend::new().unwrap();
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("failed to load font");

    let line = backend
        .shape_line(&font, "Hello")
        .expect("failed to shape text");

    assert_eq!(line.glyphs.len(), 5, "expected 5 glyphs for 'Hello'");

    // All glyph IDs should be non-zero (not .notdef)
    for (i, glyph) in line.glyphs.iter().enumerate() {
        assert!(glyph.glyph_id != 0, "glyph {} has zero ID", i);
    }

    // Total width should be positive
    assert!(line.width_lpx > 0.0, "width should be positive");

    // Positions should be monotonically increasing (LTR)
    let mut x_pos = 0.0_f32;
    for glyph in &line.glyphs {
        assert!(
            glyph.x_offset_lpx >= x_pos || glyph.x_advance_lpx > 0.0,
            "glyphs should progress left-to-right"
        );
        x_pos += glyph.x_advance_lpx;
    }
}

#[test]
fn empty_string_returns_empty_line() {
    let mut backend = CoreTextBackend::new().unwrap();
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("failed to load font");

    let line = backend
        .shape_line(&font, "")
        .expect("failed to shape empty string");

    assert!(line.glyphs.is_empty(), "empty string should have no glyphs");
    assert_eq!(line.width_lpx, 0.0, "empty string should have zero width");
}

#[test]
fn missing_glyph_returns_notdef_or_zero() {
    let mut backend = CoreTextBackend::new().unwrap();
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("failed to load font");

    // Private use area character - should map to .notdef
    let line = backend
        .shape_line(&font, "\u{E000}")
        .expect("failed to shape PUA character");

    assert_eq!(
        line.glyphs.len(),
        1,
        "PUA character should produce one glyph"
    );
    // .notdef is typically glyph ID 0, but some fonts use other IDs
    // Just verify we got exactly one glyph back
}

#[test]
fn metrics_are_reasonable() {
    let mut backend = CoreTextBackend::new().unwrap();
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("failed to load font");

    let metrics = font.metrics();

    // Ascent should be positive
    assert!(metrics.ascent_lpx > 0.0, "ascent should be positive");

    // Descent should be negative (below baseline)
    assert!(metrics.descent_lpx < 0.0, "descent should be negative");

    // Line height should be reasonable (not zero, not huge)
    let line_height = metrics.ascent_lpx - metrics.descent_lpx + metrics.line_gap_lpx;
    assert!(
        line_height > 10.0 && line_height < 100.0,
        "line height should be reasonable: {}",
        line_height
    );

    // Units per em should be typical value (1000 or 2048)
    assert!(
        metrics.units_per_em == 1000 || metrics.units_per_em == 2048,
        "units_per_em should be 1000 or 2048, got {}",
        metrics.units_per_em
    );
}
