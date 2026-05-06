//! CPU-only unit tests for GlyphCache (no GPU required).
//!
//! Tests requiring GPU are in glyph_cache_atlas_roundtrip.rs.

use slate_text::glyph_cache::pad_with_gutter;

#[test]
fn pad_with_gutter_zeros_border() {
    // 2×2 input: all 0xFF
    let src = vec![0xFFu8; 4];
    let w = 2u32;
    let h = 2u32;

    // Expected: 4×4 output with zero border
    // Row 0: [0, 0, 0, 0]
    // Row 1: [0, FF, FF, 0]
    // Row 2: [0, FF, FF, 0]
    // Row 3: [0, 0, 0, 0]
    let expected = vec![0, 0, 0, 0, 0, 0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0, 0, 0, 0];

    let padded = pad_with_gutter(&src, w, h);
    assert_eq!(padded, expected);
}
