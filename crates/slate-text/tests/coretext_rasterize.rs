//! CoreText rasterization tests (macOS only).

#![cfg(target_os = "macos")]

use slate_text::{CoreTextBackend, TEST_FONT, TextBackend};

#[test]
fn rasterizes_uppercase_h() {
    let mut backend = CoreTextBackend::new().unwrap();
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("failed to load font");

    // Shape "H" to get glyph ID
    let line = backend.shape_line(&font, "H").expect("failed to shape");
    assert_eq!(line.glyphs.len(), 1);
    let glyph_id = line.glyphs[0].glyph_id;

    // Rasterize
    let bitmap = backend
        .rasterize_glyph(&font, glyph_id, 0)
        .expect("failed to rasterize");

    // Bitmap should have non-zero dimensions
    assert!(bitmap.width > 0, "bitmap width should be positive");
    assert!(bitmap.height > 0, "bitmap height should be positive");

    // Should have at least one high-alpha pixel (the glyph rendered)
    let max_alpha = bitmap.alpha.iter().copied().max().unwrap_or(0);
    assert!(
        max_alpha > 200,
        "should have high-alpha pixels, max was {}",
        max_alpha
    );

    // Advance should be positive
    assert!(bitmap.advance_x_lpx > 0.0, "advance should be positive");
}

#[test]
fn four_variants_differ() {
    let mut backend = CoreTextBackend::new().unwrap();
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("failed to load font");

    // Shape "A" to get glyph ID
    let line = backend.shape_line(&font, "A").expect("failed to shape");
    let glyph_id = line.glyphs[0].glyph_id;

    // Rasterize all 4 variants
    let bitmaps: Vec<_> = (0..4)
        .map(|v| {
            backend
                .rasterize_glyph(&font, glyph_id, v)
                .expect("failed to rasterize variant")
        })
        .collect();

    // At least one pair of consecutive variants should differ
    let mut found_difference = false;
    for i in 0..3 {
        let a = &bitmaps[i];
        let b = &bitmaps[i + 1];

        // Compare pixel data if dimensions match
        if a.width == b.width && a.height == b.height && a.alpha.len() == b.alpha.len() {
            let differs = a.alpha.iter().zip(b.alpha.iter()).any(|(pa, pb)| pa != pb);
            if differs {
                found_difference = true;
                break;
            }
        } else {
            // Different dimensions means they definitely differ
            found_difference = true;
            break;
        }
    }

    assert!(
        found_difference,
        "at least one pair of sub-pixel variants should differ"
    );
}

#[test]
fn whitespace_returns_empty_bitmap() {
    let mut backend = CoreTextBackend::new().unwrap();
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("failed to load font");

    // Shape space to get glyph ID
    let line = backend.shape_line(&font, " ").expect("failed to shape");
    assert_eq!(line.glyphs.len(), 1);
    let glyph_id = line.glyphs[0].glyph_id;

    // Rasterize
    let bitmap = backend
        .rasterize_glyph(&font, glyph_id, 0)
        .expect("failed to rasterize space");

    // Space should have zero dimensions (no visible pixels)
    assert_eq!(bitmap.width, 0, "space width should be 0");
    assert_eq!(bitmap.height, 0, "space height should be 0");
    assert!(bitmap.alpha.is_empty(), "space alpha should be empty");

    // But advance should still be positive
    assert!(
        bitmap.advance_x_lpx > 0.0,
        "space advance should be positive"
    );
}

#[test]
fn invalid_variant_returns_error() {
    let mut backend = CoreTextBackend::new().unwrap();
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("failed to load font");

    let line = backend.shape_line(&font, "X").expect("failed to shape");
    let glyph_id = line.glyphs[0].glyph_id;

    // Variant 4 is out of range (valid: 0-3)
    let result = backend.rasterize_glyph(&font, glyph_id, 4);
    assert!(result.is_err(), "variant 4 should return error");
}

#[test]
fn bearing_values_are_reasonable() {
    let mut backend = CoreTextBackend::new().unwrap();
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("failed to load font");

    let line = backend.shape_line(&font, "H").expect("failed to shape");
    let glyph_id = line.glyphs[0].glyph_id;

    let bitmap = backend
        .rasterize_glyph(&font, glyph_id, 0)
        .expect("failed to rasterize");

    // Bearing Y should be positive (glyph sits above baseline)
    assert!(
        bitmap.bearing_y_lpx > 0.0,
        "bearing_y should be positive for 'H', got {}",
        bitmap.bearing_y_lpx
    );

    // Bearing X can be negative, zero, or positive depending on glyph
    // Just verify it's within reasonable bounds
    assert!(
        bitmap.bearing_x_lpx.abs() < 20.0,
        "bearing_x should be reasonable, got {}",
        bitmap.bearing_x_lpx
    );
}
