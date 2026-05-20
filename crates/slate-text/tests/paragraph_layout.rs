//! Tests for multi-line paragraph layout.

use slate_text::error::TextError;
use slate_text::font_handle::FontHandle;
use slate_text::types::{
    FontDescriptor, FontId, FontMetrics, GlyphBitmap, GlyphBounds, ShapedGlyph, ShapedLine,
};
use slate_text::{Font, TextBackend};
use slate_text::{
    TextAlignment, compute_alignment_offset, greedy_wrap, shape_words, truncate_with_ellipsis,
    wrap_shaped_words,
};
use std::cell::Cell;

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
            handle: FontHandle::from_face_id(0x1000, 16.0, 1.0),
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
        let mut pen = 0.0f32;
        let glyphs: Vec<ShapedGlyph> = text
            .chars()
            .enumerate()
            .map(|(i, c)| {
                let advance = if c == ' ' { 5.0 } else { 10.0 };
                let g = ShapedGlyph {
                    glyph_id: i as u32,
                    font_id: FontId::PRIMARY,
                    font_handle: Default::default(),
                    x_advance_lpx: advance,
                    position_lpx: [pen, 0.0],
                    cluster: 0,
                };
                pen += advance;
                g
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

/// `MockBackend` variant that counts `shape_line` calls via interior
/// mutability (`shape_line` takes `&self`). Used to prove that re-fitting
/// pre-shaped words to a new width triggers no additional shaping.
struct CountingBackend {
    shape_calls: Cell<usize>,
}

impl CountingBackend {
    fn new() -> Self {
        Self {
            shape_calls: Cell::new(0),
        }
    }
    fn calls(&self) -> usize {
        self.shape_calls.get()
    }
}

impl TextBackend for CountingBackend {
    type Font = MockFont;

    fn load_font(
        &mut self,
        _family: &str,
        _size_lpx: f32,
        _scale: f32,
    ) -> Result<Self::Font, TextError> {
        MockBackend.load_font("mock", 16.0, 1.0)
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
        self.shape_calls.set(self.shape_calls.get() + 1);
        MockBackend.shape_line(font, text)
    }

    fn rasterize_glyph(
        &self,
        font: &Self::Font,
        glyph_id: u32,
        variant: u8,
    ) -> Result<GlyphBitmap, TextError> {
        MockBackend.rasterize_glyph(font, glyph_id, variant)
    }

    fn glyph_raster_bounds(
        &self,
        font: &Self::Font,
        glyph_id: u32,
    ) -> Result<GlyphBounds, TextError> {
        MockBackend.glyph_raster_bounds(font, glyph_id)
    }

    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError> {
        Ok(vec![])
    }
}

#[test]
fn rewrap_at_new_width_does_no_reshaping() {
    let mut backend = CountingBackend::new();
    let font = backend.load_font("mock", 16.0, 1.0).unwrap();
    let line_height = 18.0;

    // Shape words once.
    let (words, space_width) = shape_words(&backend, &font, "Hello world test again").unwrap();
    let after_shape = backend.calls();
    assert!(after_shape > 0, "shaping should call shape_line");

    // Fit at width A — pure arithmetic, no shaping.
    let lines_a = wrap_shaped_words(&words, space_width, line_height, 80.0);
    assert_eq!(
        backend.calls(),
        after_shape,
        "first wrap must add zero shape_line calls"
    );

    // Re-fit the SAME pre-shaped words at width B — still zero shaping.
    let lines_b = wrap_shaped_words(&words, space_width, line_height, 40.0);
    assert_eq!(
        backend.calls(),
        after_shape,
        "re-wrap at a new width must add zero shape_line calls"
    );

    // Sanity: narrower width yields at least as many lines.
    assert!(lines_b.len() >= lines_a.len());
}

#[test]
fn arithmetic_wrap_matches_greedy_wrap() {
    let mut backend = MockBackend;
    let font = backend.load_font("mock", 16.0, 1.0).unwrap();
    // greedy_wrap derives line_height from font metrics: 12 - (-4) + 2 = 18.
    let line_height = 18.0;

    for (text, width) in [
        ("Hello world test", 80.0),
        ("one two three", 50.0),
        ("singleword", 200.0),
        ("a b c d e f g", 35.0),
        ("overflowingsingleword tiny", 30.0),
    ] {
        let reference = greedy_wrap(&backend, &font, text, width).unwrap();
        let (words, space_width) = shape_words(&backend, &font, text).unwrap();
        let arithmetic = wrap_shaped_words(&words, space_width, line_height, width);

        assert_eq!(
            arithmetic.len(),
            reference.len(),
            "line count mismatch for {text:?} @ {width}"
        );
        for (i, (a, r)) in arithmetic.iter().zip(reference.iter()).enumerate() {
            assert!(
                (a.width_lpx - r.width_lpx).abs() < 0.01,
                "line {i} width mismatch for {text:?} @ {width}: {} vs {}",
                a.width_lpx,
                r.width_lpx
            );
            assert!(
                (a.y_offset_lpx - r.y_offset_lpx).abs() < 0.01,
                "line {i} y_offset mismatch for {text:?} @ {width}: {} vs {}",
                a.y_offset_lpx,
                r.y_offset_lpx
            );
        }
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
    for (i, line) in lines.iter().enumerate().skip(1) {
        let expected_y = expected_line_height * i as f32;
        assert!(
            (line.y_offset_lpx - expected_y).abs() < 0.01,
            "line {} y_offset should be {}, got {}",
            i,
            expected_y,
            line.y_offset_lpx
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
fn shape_words_emits_items_in_order() {
    let mut backend = MockBackend;
    let font = backend.load_font("mock", 16.0, 1.0).unwrap();

    // "a     b": word "a", a 5-space run, word "b" — every space byte preserved.
    let (items, _space_width) = shape_words(&backend, &font, "a     b").unwrap();
    assert_eq!(items.len(), 3, "word, space-run, word");
    assert!(!items[0].is_space_run);
    assert_eq!(items[0].source_byte_range, 0..1);
    assert!(items[1].is_space_run);
    assert_eq!(items[1].source_byte_range, 1..6);
    assert_eq!(items[1].glyphs.len(), 5, "one glyph per preserved space byte");
    assert!(!items[2].is_space_run);
    assert_eq!(items[2].source_byte_range, 6..7);

    // Leading-only whitespace: zero words + one space run, still addressable.
    let (items, _) = shape_words(&backend, &font, "   abc").unwrap();
    assert!(items[0].is_space_run);
    assert_eq!(items[0].source_byte_range, 0..3);
    assert!(!items[1].is_space_run);
    assert_eq!(items[1].source_byte_range, 3..6);

    // Trailing-only whitespace.
    let (items, _) = shape_words(&backend, &font, "abc   ").unwrap();
    assert!(!items[0].is_space_run);
    assert_eq!(items[0].source_byte_range, 0..3);
    assert!(items[1].is_space_run);
    assert_eq!(items[1].source_byte_range, 3..6);

    // Pure whitespace: a single space run, nothing else.
    let (items, _) = shape_words(&backend, &font, "     ").unwrap();
    assert_eq!(items.len(), 1);
    assert!(items[0].is_space_run);
    assert_eq!(items[0].source_byte_range, 0..5);

    // Empty string: no items.
    let (items, _) = shape_words(&backend, &font, "").unwrap();
    assert!(items.is_empty());
}

#[test]
fn single_space_prose_layout_unchanged() {
    // Regression guard: preserving spaces must not shift single-space widths.
    // "Hello world test" each char 10, space 5 → identical to the pre-fix join.
    let mut backend = MockBackend;
    let font = backend.load_font("mock", 16.0, 1.0).unwrap();
    let lines = greedy_wrap(&backend, &font, "Hello world test", 80.0).unwrap();
    assert_eq!(lines.len(), 3, "single-space wrap unchanged");
    assert!((lines[0].width_lpx - 50.0).abs() < 0.01);
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
