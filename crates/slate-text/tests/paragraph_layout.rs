//! Tests for multi-line paragraph layout.

use slate_text::backend::{Font, TextBackend};
use slate_text::error::TextError;
use slate_text::font_handle::FontHandle;
use slate_text::types::{
    FontDescriptor, FontId, FontMetrics, GlyphBitmap, GlyphBounds, ShapedGlyph, ShapedLine,
};
use slate_text::{TextAlignment, compute_alignment_offset, greedy_wrap, truncate_with_ellipsis};

/// Mock font for testing.
struct MockFont {
    handle: FontHandle,
    metrics: FontMetrics,
}

impl Font for MockFont {
    fn handle(&self) -> FontHandle {
        self.handle
    }

    fn metrics(&self) -> FontMetrics {
        self.metrics
    }

    fn size_lpx(&self) -> f32 {
        16.0
    }

    fn scale(&self) -> f32 {
        1.0
    }
}

/// Mock backend with predictable glyph widths.
/// Each character is 10 lpx wide, space is 5 lpx.
struct MockBackend;

impl TextBackend for MockBackend {
    type Font = MockFont;

    fn load_font(
        &mut self,
        _family: &str,
        _size_lpx: f32,
        _scale: f32,
    ) -> Result<Self::Font, TextError> {
        Ok(MockFont {
            handle: FontHandle::from_ptr_size_scale(0x1000 as *const (), 16.0, 1.0),
            metrics: FontMetrics {
                ascent_lpx: 12.0,
                descent_lpx: -4.0,
                line_gap_lpx: 2.0,
                x_height_lpx: 8.0,
                cap_height_lpx: 10.0,
                units_per_em: 2048,
            },
        })
    }

    fn load_font_from_bytes(
        &mut self,
        _bytes: &'static [u8],
        _size_lpx: f32,
        _scale: f32,
    ) -> Result<Self::Font, TextError> {
        self.load_font("mock", 16.0, 1.0)
    }

    fn shape_line(&self, font: &Self::Font, text: &str) -> Result<ShapedLine, TextError> {
        // Each non-space char is 10 lpx, space is 5 lpx
        let glyphs: Vec<ShapedGlyph> = text
            .chars()
            .enumerate()
            .map(|(i, c)| {
                let advance = if c == ' ' { 5.0 } else { 10.0 };
                ShapedGlyph {
                    glyph_id: i as u32,
                    font_id: FontId::PRIMARY,
                    x_advance_lpx: advance,
                    x_offset_lpx: 0.0,
                    y_offset_lpx: 0.0,
                }
            })
            .collect();
        let width: f32 = glyphs.iter().map(|g| g.x_advance_lpx).sum();
        Ok(ShapedLine {
            glyphs,
            width_lpx: width,
            ascent_lpx: font.metrics.ascent_lpx,
            descent_lpx: font.metrics.descent_lpx,
            y_offset_lpx: 0.0,
        })
    }

    fn rasterize_glyph(
        &self,
        _font: &Self::Font,
        _glyph_id: u32,
        _variant: u8,
    ) -> Result<GlyphBitmap, TextError> {
        Ok(GlyphBitmap {
            width: 8,
            height: 12,
            bearing_x_lpx: 1.0,
            bearing_y_lpx: 10.0,
            advance_x_lpx: 10.0,
            alpha: vec![0xFF; 96],
        })
    }

    fn glyph_raster_bounds(
        &self,
        _font: &Self::Font,
        _glyph_id: u32,
    ) -> Result<GlyphBounds, TextError> {
        Ok(GlyphBounds {
            width: 8,
            height: 12,
        })
    }

    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError> {
        Ok(vec![])
    }
}

#[test]
fn wrap_hello_world_at_narrow_width() {
    // "Hello world" with each char=10lpx, space=5lpx
    // "Hello" = 50lpx, "world" = 50lpx
    // With max_width=80, should wrap to two lines
    let mut backend = MockBackend;
    let font = backend.load_font("mock", 16.0, 1.0).unwrap();

    let lines = greedy_wrap(&backend, &font, "Hello world test", 80.0).unwrap();

    // "Hello" (50) fits at 80
    // Adding " world" would be 50 + 5 + 50 = 105 > 80, so wrap
    // Line 1: "Hello" (50)
    // Line 2: "world" (50)
    // Adding " test" would be 50 + 5 + 40 = 95 > 80, so wrap
    // Line 3: "test" (40)
    assert_eq!(lines.len(), 3, "should wrap into 3 lines");
    assert_eq!(lines[0].y_offset_lpx, 0.0);
    assert!(
        lines[1].y_offset_lpx > 0.0,
        "second line should have positive y_offset"
    );
    assert!(
        lines[2].y_offset_lpx > lines[1].y_offset_lpx,
        "third line should be lower"
    );
}

#[test]
fn y_offsets_use_line_height() {
    let mut backend = MockBackend;
    let font = backend.load_font("mock", 16.0, 1.0).unwrap();

    let lines = greedy_wrap(&backend, &font, "one two three", 50.0).unwrap();

    // Each word is ~30-50 lpx, so with max=50, each word goes on its own line
    assert!(lines.len() >= 2);

    // Line height = ascent - descent + line_gap = 12 - (-4) + 2 = 18
    let expected_line_height = 18.0;
    for i in 1..lines.len() {
        let expected_y = expected_line_height * i as f32;
        assert!(
            (lines[i].y_offset_lpx - expected_y).abs() < 0.01,
            "line {} y_offset should be {}, got {}",
            i,
            expected_y,
            lines[i].y_offset_lpx
        );
    }
}

#[test]
fn center_alignment_computes_correct_offset() {
    // Line width 100, container 200 → offset should be 50
    let offset = compute_alignment_offset(100.0, 200.0, TextAlignment::Center);
    assert!((offset - 50.0).abs() < 0.01);
}

#[test]
fn right_alignment_computes_correct_offset() {
    // Line width 100, container 200 → offset should be 100
    let offset = compute_alignment_offset(100.0, 200.0, TextAlignment::Right);
    assert!((offset - 100.0).abs() < 0.01);
}

#[test]
fn empty_text_returns_empty_vec() {
    let mut backend = MockBackend;
    let font = backend.load_font("mock", 16.0, 1.0).unwrap();

    let lines = greedy_wrap(&backend, &font, "", 100.0).unwrap();
    assert!(lines.is_empty());
}

#[test]
fn single_word_fits_on_one_line() {
    let mut backend = MockBackend;
    let font = backend.load_font("mock", 16.0, 1.0).unwrap();

    let lines = greedy_wrap(&backend, &font, "Hello", 200.0).unwrap();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].y_offset_lpx, 0.0);
}

#[test]
fn shape_paragraph_trait_method_works() {
    let mut backend = MockBackend;
    let font = backend.load_font("mock", 16.0, 1.0).unwrap();

    // Use the trait method directly
    let lines = backend.shape_paragraph(&font, "Hello world", 80.0).unwrap();
    assert!(lines.len() >= 2);
}

#[test]
fn truncate_line_that_fits() {
    let backend = MockBackend;
    let mut backend_mut = MockBackend;
    let font = backend_mut.load_font("mock", 16.0, 1.0).unwrap();

    // "Hello" = 50 lpx, max = 100 → no truncation
    let shaped = backend.shape_line(&font, "Hello").unwrap();
    let result = truncate_with_ellipsis(&backend, &font, &shaped, 100.0).unwrap();

    assert_eq!(result.glyphs.len(), shaped.glyphs.len());
    assert!((result.width_lpx - shaped.width_lpx).abs() < 0.01);
}

#[test]
fn truncate_line_adds_ellipsis() {
    let backend = MockBackend;
    let mut backend_mut = MockBackend;
    let font = backend_mut.load_font("mock", 16.0, 1.0).unwrap();

    // "Hello world" = 105 lpx (10*10 + 5), max = 80
    // "..." = 30 lpx, so target = 50 lpx → keeps "Hello" (50)
    let shaped = backend.shape_line(&font, "Hello world").unwrap();
    let result = truncate_with_ellipsis(&backend, &font, &shaped, 80.0).unwrap();

    // Should have fewer glyphs than original + ellipsis glyphs
    assert!(result.glyphs.len() < shaped.glyphs.len() + 3);
    assert!(result.width_lpx <= 80.0);
}

#[test]
fn truncate_very_narrow_returns_ellipsis_only() {
    let backend = MockBackend;
    let mut backend_mut = MockBackend;
    let font = backend_mut.load_font("mock", 16.0, 1.0).unwrap();

    // "Hello" = 50 lpx, max = 25 → can't fit even one char + ellipsis
    let shaped = backend.shape_line(&font, "Hello").unwrap();
    let result = truncate_with_ellipsis(&backend, &font, &shaped, 25.0).unwrap();

    // Should return just ellipsis (3 glyphs for "...")
    assert_eq!(result.glyphs.len(), 3);
}
