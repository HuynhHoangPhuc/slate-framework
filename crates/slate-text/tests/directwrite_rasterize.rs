//! DirectWrite rasterization tests (Windows only).

#![cfg(target_os = "windows")]

use slate_text::{DirectWriteBackend, TEST_FONT, TextBackend};

#[test]
fn rasterizes_uppercase_h() {
    let mut backend = DirectWriteBackend::new().expect("backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("font load");

    // Shape "H" to get glyph ID
    let shaped = backend.shape_line(&font, "H").expect("shape");
    assert_eq!(shaped.glyphs.len(), 1);
    let glyph_id = shaped.glyphs[0].glyph_id;

    // Rasterize
    let bitmap = backend
        .rasterize_glyph(&font, glyph_id, 0)
        .expect("rasterize");

    assert!(bitmap.width > 0, "bitmap should have width");
    assert!(bitmap.height > 0, "bitmap should have height");
    assert!(
        bitmap.alpha.iter().any(|&a| a > 200),
        "bitmap should have at least one opaque pixel"
    );
}

#[test]
fn four_variants_differ() {
    let mut backend = DirectWriteBackend::new().expect("backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("font load");

    // Shape "A" to get glyph ID
    let shaped = backend.shape_line(&font, "A").expect("shape");
    let glyph_id = shaped.glyphs[0].glyph_id;

    // Rasterize at all 4 variants
    let bitmaps: Vec<_> = (0..4)
        .map(|v| {
            backend
                .rasterize_glyph(&font, glyph_id, v)
                .expect("rasterize")
        })
        .collect();

    // Check that at least one pair differs (sub-pixel offset should be visible)
    let mut found_diff = false;
    for i in 0..3 {
        if bitmaps[i].alpha != bitmaps[i + 1].alpha {
            found_diff = true;
            break;
        }
    }

    assert!(
        found_diff,
        "at least two consecutive variants should differ due to sub-pixel offset"
    );
}

#[test]
fn whitespace_returns_empty_bitmap() {
    let mut backend = DirectWriteBackend::new().expect("backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("font load");

    // Shape space to get glyph ID
    let shaped = backend.shape_line(&font, " ").expect("shape");
    let glyph_id = shaped.glyphs[0].glyph_id;

    // Rasterize space
    let bitmap = backend
        .rasterize_glyph(&font, glyph_id, 0)
        .expect("rasterize");

    assert_eq!(bitmap.width, 0, "space glyph should have zero width");
    assert_eq!(bitmap.height, 0, "space glyph should have zero height");
    assert!(
        bitmap.alpha.is_empty(),
        "space glyph should have empty alpha"
    );
    assert!(
        bitmap.advance_x_lpx > 0.0,
        "space should still have advance"
    );
}

#[test]
fn variant_out_of_range_returns_error() {
    let mut backend = DirectWriteBackend::new().expect("backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("font load");

    let shaped = backend.shape_line(&font, "X").expect("shape");
    let glyph_id = shaped.glyphs[0].glyph_id;

    // Variant 4 should fail
    let result = backend.rasterize_glyph(&font, glyph_id, 4);
    assert!(result.is_err(), "variant 4 should return error");
}

#[test]
fn rasterize_multiple_glyphs() {
    let mut backend = DirectWriteBackend::new().expect("backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 12.0, 1.5)
        .expect("font load");

    let shaped = backend.shape_line(&font, "Test").expect("shape");

    for glyph in &shaped.glyphs {
        let bitmap = backend
            .rasterize_glyph(&font, glyph.glyph_id, 0)
            .expect("rasterize");

        // All printable glyphs should have content (except space, but "Test" has none)
        assert!(bitmap.width > 0 && bitmap.height > 0);
    }
}
