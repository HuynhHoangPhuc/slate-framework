//! CPU-only unit tests for GlyphCache (no GPU required).

use slate_text::backend::{Font, TextBackend};
use slate_text::error::TextError;
use slate_text::font_handle::FontHandle;
use slate_text::glyph_cache::GlyphCache;
use slate_text::types::{FontMetrics, GlyphBitmap, ShapedGlyph, ShapedLine};

/// Mock font for testing.
struct MockFont {
    handle: FontHandle,
    size_lpx: f32,
    scale: f32,
}

impl Font for MockFont {
    fn handle(&self) -> FontHandle {
        self.handle
    }

    fn metrics(&self) -> FontMetrics {
        FontMetrics {
            ascent_lpx: 12.0,
            descent_lpx: -3.0,
            line_gap_lpx: 1.0,
            x_height_lpx: 8.0,
            cap_height_lpx: 10.0,
            units_per_em: 2048,
        }
    }

    fn size_lpx(&self) -> f32 {
        self.size_lpx
    }

    fn scale(&self) -> f32 {
        self.scale
    }
}

/// Mock backend that produces predictable bitmaps.
struct MockBackend {
    /// If true, glyph_id 0 returns an empty bitmap (whitespace).
    whitespace_at_zero: bool,
}

impl MockBackend {
    fn new() -> Self {
        Self {
            whitespace_at_zero: false,
        }
    }

    fn with_whitespace_at_zero() -> Self {
        Self {
            whitespace_at_zero: true,
        }
    }
}

impl TextBackend for MockBackend {
    type Font = MockFont;

    fn load_font(
        &mut self,
        _family: &str,
        size_lpx: f32,
        scale: f32,
    ) -> Result<Self::Font, TextError> {
        // Use a predictable pointer for testing
        let ptr = 0x12345678 as *const u8;
        Ok(MockFont {
            handle: FontHandle::from_ptr_size_scale(ptr, size_lpx, scale),
            size_lpx,
            scale,
        })
    }

    fn load_font_from_bytes(
        &mut self,
        bytes: &'static [u8],
        size_lpx: f32,
        scale: f32,
    ) -> Result<Self::Font, TextError> {
        Ok(MockFont {
            handle: FontHandle::from_ptr_size_scale(bytes.as_ptr(), size_lpx, scale),
            size_lpx,
            scale,
        })
    }

    fn shape_line(&self, _font: &Self::Font, text: &str) -> Result<ShapedLine, TextError> {
        // Each character gets a glyph with ID = char index
        let glyphs: Vec<ShapedGlyph> = text
            .chars()
            .enumerate()
            .map(|(i, _)| ShapedGlyph {
                glyph_id: i as u32,
                x_advance_lpx: 10.0,
                x_offset_lpx: 0.0,
                y_offset_lpx: 0.0,
            })
            .collect();
        let width = glyphs.len() as f32 * 10.0;
        Ok(ShapedLine {
            glyphs,
            width_lpx: width,
            ascent_lpx: 12.0,
            descent_lpx: -3.0,
        })
    }

    fn rasterize_glyph(
        &self,
        _font: &Self::Font,
        glyph_id: u32,
        variant: u8,
    ) -> Result<GlyphBitmap, TextError> {
        // Whitespace test: glyph_id 0 returns empty bitmap
        if self.whitespace_at_zero && glyph_id == 0 {
            return Ok(GlyphBitmap {
                width: 0,
                height: 0,
                bearing_x_lpx: 0.0,
                bearing_y_lpx: 0.0,
                advance_x_lpx: 10.0,
                alpha: vec![],
            });
        }

        // Produce a predictable bitmap: width = glyph_id + 1, height = variant + 1
        let w = (glyph_id + 1).min(16);
        let h = (variant as u32 + 1).min(16);
        let alpha = vec![0xFF; (w * h) as usize];
        Ok(GlyphBitmap {
            width: w,
            height: h,
            bearing_x_lpx: 1.0,
            bearing_y_lpx: (h as f32) - 1.0,
            advance_x_lpx: (w as f32) + 2.0,
            alpha,
        })
    }
}

#[test]
fn materialize_creates_pending_for_misses() {
    let mut cache = GlyphCache::new();
    let mut backend = MockBackend::new();
    let font = backend.load_font("Test", 16.0, 1.0).unwrap();
    let shaped = backend.shape_line(&font, "abc").unwrap();

    cache.materialize(&backend, &font, &shaped).unwrap();

    // 3 glyphs × 4 variants = 12 pending
    assert_eq!(cache.pending_len(), 12);
}

#[test]
fn materialize_skips_already_pending() {
    let mut cache = GlyphCache::new();
    let mut backend = MockBackend::new();
    let font = backend.load_font("Test", 16.0, 1.0).unwrap();
    let shaped = backend.shape_line(&font, "abc").unwrap();

    cache.materialize(&backend, &font, &shaped).unwrap();
    let count1 = cache.pending_len();

    // Calling again should not grow pending (dedup check)
    cache.materialize(&backend, &font, &shaped).unwrap();
    assert_eq!(cache.pending_len(), count1);
}

#[test]
fn materialize_skips_empty_bitmaps() {
    let mut cache = GlyphCache::new();
    let backend = MockBackend::with_whitespace_at_zero();
    // Shape "a" which maps to glyph_id 0 (whitespace)
    let font = MockFont {
        handle: FontHandle::from_ptr_size_scale(0x1000 as *const u8, 16.0, 1.0),
        size_lpx: 16.0,
        scale: 1.0,
    };
    let shaped = ShapedLine {
        glyphs: vec![ShapedGlyph {
            glyph_id: 0, // Will return empty bitmap
            x_advance_lpx: 10.0,
            x_offset_lpx: 0.0,
            y_offset_lpx: 0.0,
        }],
        width_lpx: 10.0,
        ascent_lpx: 12.0,
        descent_lpx: -3.0,
    };

    cache.materialize(&backend, &font, &shaped).unwrap();

    // Empty bitmaps should be skipped
    assert_eq!(cache.pending_len(), 0);
}

#[test]
fn pad_with_gutter_zeros_border() {
    // Access internal function via module re-export trick: test the padded buffer
    // by observing the behavior through the public API (flush).
    // For direct testing, we'll verify the contract manually.

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

    let padded = slate_text::glyph_cache::pad_with_gutter(&src, w, h);
    assert_eq!(padded, expected);
}
