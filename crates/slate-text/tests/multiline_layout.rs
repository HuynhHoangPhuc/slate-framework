//! Tests for byte-aware multi-line layout (`shape_document` / `wrap_document`).
//!
//! The mock backend below sets each glyph's `cluster` to the source byte offset
//! of its character (real backends do the same via the HarfBuzz convention), so
//! the over-wide-word grapheme break can be verified by byte offset.

use std::cell::Cell;

use slate_text::error::TextError;
use slate_text::font_handle::FontHandle;
use slate_text::types::{
    FontDescriptor, FontId, FontMetrics, GlyphBitmap, GlyphBounds, ShapedGlyph, ShapedLine,
};
use slate_text::{Font, TextBackend, shape_document, shape_words, wrap_document};

// ── Mock font/backend ────────────────────────────────────────────────────────

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

/// Non-space char = 10 lpx, space = 5 lpx. `cluster` = byte offset of the char
/// in the shaped string; `line_height = 12 - (-4) + 2 = 18`.
struct MockBackend;

impl MockBackend {
    fn font() -> MockFont {
        MockFont {
            handle: FontHandle::from_face_id(0x1000, 16.0, 1.0),
            metrics: FontMetrics {
                ascent_lpx: 12.0,
                descent_lpx: -4.0,
                line_gap_lpx: 2.0,
                x_height_lpx: 8.0,
                cap_height_lpx: 10.0,
                units_per_em: 2048,
            },
        }
    }
}

fn shape_line_impl(font: &MockFont, text: &str) -> ShapedLine {
    let mut pen = 0.0f32;
    let mut byte = 0usize;
    let glyphs: Vec<ShapedGlyph> = text
        .chars()
        .map(|c| {
            let advance = if c == ' ' { 5.0 } else { 10.0 };
            let g = ShapedGlyph {
                glyph_id: byte as u32,
                font_id: FontId::PRIMARY,
                font_handle: Default::default(),
                x_advance_lpx: advance,
                position_lpx: [pen, 0.0],
                cluster: byte as u32,
            };
            pen += advance;
            byte += c.len_utf8();
            g
        })
        .collect();
    let width: f32 = glyphs.iter().map(|g| g.x_advance_lpx).sum();
    ShapedLine {
        glyphs,
        width_lpx: width,
        ascent_lpx: font.metrics.ascent_lpx,
        descent_lpx: font.metrics.descent_lpx,
        y_offset_lpx: 0.0,
    }
}

impl TextBackend for MockBackend {
    type Font = MockFont;

    fn load_font(&mut self, _f: &str, _s: f32, _sc: f32) -> Result<Self::Font, TextError> {
        Ok(MockBackend::font())
    }
    fn load_font_from_bytes(
        &mut self,
        _b: &'static [u8],
        _s: f32,
        _sc: f32,
    ) -> Result<Self::Font, TextError> {
        Ok(MockBackend::font())
    }
    fn shape_line(&self, font: &Self::Font, text: &str) -> Result<ShapedLine, TextError> {
        Ok(shape_line_impl(font, text))
    }
    fn rasterize_glyph(&self, _f: &Self::Font, _g: u32, _v: u8) -> Result<GlyphBitmap, TextError> {
        Ok(GlyphBitmap {
            width: 8,
            height: 12,
            bearing_x_lpx: 1.0,
            bearing_y_lpx: 10.0,
            advance_x_lpx: 10.0,
            alpha: vec![0xFF; 96],
        })
    }
    fn glyph_raster_bounds(&self, _f: &Self::Font, _g: u32) -> Result<GlyphBounds, TextError> {
        Ok(GlyphBounds {
            width: 8,
            height: 12,
        })
    }
    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError> {
        Ok(vec![])
    }
}

/// `shape_line`-counting backend (interior mutability) to prove re-fit shapes
/// nothing.
struct CountingBackend {
    calls: Cell<usize>,
}

impl TextBackend for CountingBackend {
    type Font = MockFont;
    fn load_font(&mut self, _f: &str, _s: f32, _sc: f32) -> Result<Self::Font, TextError> {
        Ok(MockBackend::font())
    }
    fn load_font_from_bytes(
        &mut self,
        _b: &'static [u8],
        _s: f32,
        _sc: f32,
    ) -> Result<Self::Font, TextError> {
        Ok(MockBackend::font())
    }
    fn shape_line(&self, font: &Self::Font, text: &str) -> Result<ShapedLine, TextError> {
        self.calls.set(self.calls.get() + 1);
        Ok(shape_line_impl(font, text))
    }
    fn rasterize_glyph(&self, _f: &Self::Font, _g: u32, _v: u8) -> Result<GlyphBitmap, TextError> {
        Ok(GlyphBitmap {
            width: 8,
            height: 12,
            bearing_x_lpx: 1.0,
            bearing_y_lpx: 10.0,
            advance_x_lpx: 10.0,
            alpha: vec![0xFF; 96],
        })
    }
    fn glyph_raster_bounds(&self, _f: &Self::Font, _g: u32) -> Result<GlyphBounds, TextError> {
        Ok(GlyphBounds {
            width: 8,
            height: 12,
        })
    }
    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError> {
        Ok(vec![])
    }
}

const LINE_HEIGHT: f32 = 18.0;

// ── Test 1: shape_words populates source_byte_range ───────────────────────────

#[test]
fn shape_words_records_source_byte_ranges() {
    let mut backend = MockBackend;
    let font = backend.load_font("mock", 16.0, 1.0).unwrap();

    // ASCII
    let (words, _) = shape_words(&backend, &font, "ab cd").unwrap();
    assert_eq!(words.len(), 2);
    assert_eq!(words[0].source_byte_range, 0..2);
    assert_eq!(words[1].source_byte_range, 3..5);

    // CJK (3 bytes each)
    let (words, _) = shape_words(&backend, &font, "你 好").unwrap();
    assert_eq!(words[0].source_byte_range, 0..3);
    assert_eq!(words[1].source_byte_range, 4..7);

    // Emoji (4 bytes)
    let (words, _) = shape_words(&backend, &font, "😀 x").unwrap();
    assert_eq!(words[0].source_byte_range, 0..4);
    assert_eq!(words[1].source_byte_range, 5..6);
}

// ── Test 2: byte-aware wrap → contiguous ranges covering 0..len ───────────────

#[test]
fn wrap_yields_contiguous_byte_ranges() {
    let backend = MockBackend;
    let font = MockBackend::font();
    let text = "aa bb cc"; // each word 20 lpx, space 5
    let doc = shape_document(&backend, &font, text).unwrap();

    // width 50: "aa bb" (45) fits, +cc would be 70 → 2 lines.
    let layout = wrap_document(&doc, 50.0);
    assert_eq!(layout.lines.len(), 2);
    assert_eq!(layout.lines[0].byte_start, 0);
    assert_eq!(layout.lines[0].byte_end, 6); // "aa bb " incl joining space
    assert_eq!(layout.lines[1].byte_start, 6);
    assert_eq!(layout.lines[1].byte_end, text.len()); // 8

    // Contiguous + full coverage.
    assert_eq!(layout.lines[0].byte_end, layout.lines[1].byte_start);
    assert_eq!(layout.lines[0].byte_start, 0);
    assert_eq!(layout.lines.last().unwrap().byte_end, text.len());
}

// ── Test 3: hard newline + empty paragraph ────────────────────────────────────

#[test]
fn hard_newline_splits_lines() {
    let backend = MockBackend;
    let font = MockBackend::font();
    let doc = shape_document(&backend, &font, "a\nb").unwrap();
    let layout = wrap_document(&doc, 1000.0);

    assert_eq!(layout.lines.len(), 2);
    assert_eq!(layout.lines[0].byte_start, 0);
    assert_eq!(layout.lines[0].byte_end, 2); // "a" + folded '\n'
    assert_eq!(layout.lines[1].byte_start, 2);
    assert_eq!(layout.lines[1].byte_end, 3);
    assert_eq!(layout.lines[0].line.y_offset_lpx, 0.0);
    assert_eq!(layout.lines[1].line.y_offset_lpx, LINE_HEIGHT);
}

#[test]
fn empty_paragraph_is_full_height_blank_line() {
    let backend = MockBackend;
    let font = MockBackend::font();
    let text = "a\n\nb"; // bytes: a=0 \n=1 \n=2 b=3
    let doc = shape_document(&backend, &font, text).unwrap();
    let layout = wrap_document(&doc, 1000.0);

    assert_eq!(layout.lines.len(), 3);
    // Middle line is the empty paragraph: zero glyphs but full line height.
    assert!(layout.lines[1].line.glyphs.is_empty());
    assert_eq!(layout.lines[1].byte_start, 2);
    assert_eq!(layout.lines[1].byte_end, 3);
    // Cumulative y is monotonic.
    assert!(layout.lines[0].line.y_offset_lpx < layout.lines[1].line.y_offset_lpx);
    assert!(layout.lines[1].line.y_offset_lpx < layout.lines[2].line.y_offset_lpx);
    // Coverage is gap-free and total.
    assert_eq!(layout.lines[0].byte_start, 0);
    assert_eq!(layout.lines.last().unwrap().byte_end, text.len());
}

// ── Test 4: auto-height ───────────────────────────────────────────────────────

#[test]
fn total_height_is_sum_of_line_heights() {
    let backend = MockBackend;
    let font = MockBackend::font();
    let doc = shape_document(&backend, &font, "aa bb cc").unwrap();
    let layout = wrap_document(&doc, 50.0); // 2 lines

    assert_eq!(layout.line_height_lpx, LINE_HEIGHT);
    assert_eq!(
        layout.total_height_lpx,
        layout.lines.len() as f32 * LINE_HEIGHT
    );
    assert_eq!(layout.total_height_lpx, 2.0 * LINE_HEIGHT);
}

// ── Test 5: re-wrap does zero re-shaping ──────────────────────────────────────

#[test]
fn rewrap_at_new_width_does_no_reshaping() {
    let backend = CountingBackend {
        calls: Cell::new(0),
    };
    let font = MockBackend::font();

    let doc = shape_document(&backend, &font, "Hello world test again").unwrap();
    let after_shape = backend.calls.get();
    assert!(after_shape > 0, "shaping should call shape_line");

    let lines_a = wrap_document(&doc, 80.0);
    assert_eq!(
        backend.calls.get(),
        after_shape,
        "first wrap must add zero shape_line calls"
    );

    let lines_b = wrap_document(&doc, 40.0);
    assert_eq!(
        backend.calls.get(),
        after_shape,
        "re-wrap at a new width must add zero shape_line calls"
    );

    assert!(lines_b.lines.len() >= lines_a.lines.len());
}

// ── Test 6: over-width word breaks at grapheme boundaries ─────────────────────

#[test]
fn over_width_word_breaks_at_grapheme_boundaries() {
    let backend = MockBackend;
    let font = MockBackend::font();
    let text = "aaaaa"; // one 50-lpx word
    let doc = shape_document(&backend, &font, text).unwrap();

    // max_width 25: 2 chars (20) fit, a 3rd (30) overflows → break.
    let layout = wrap_document(&doc, 25.0);
    assert!(
        layout.lines.len() >= 2,
        "over-wide word must break into >=2 lines"
    );
    for line in &layout.lines {
        assert!(
            line.line.width_lpx <= 25.0,
            "no broken piece may exceed max_width: {}",
            line.line.width_lpx
        );
    }
    // Byte ranges stay contiguous and cover the whole word.
    assert_eq!(layout.lines[0].byte_start, 0);
    assert_eq!(layout.lines.last().unwrap().byte_end, text.len());
    for w in layout.lines.windows(2) {
        assert_eq!(w[0].byte_end, w[1].byte_start);
    }
}

// ── Test 7: caret-at-boundary resolves to next line ───────────────────────────

#[test]
fn line_for_byte_at_wrap_boundary_picks_next_line() {
    let backend = MockBackend;
    let font = MockBackend::font();
    let text = "aa bb cc";
    let doc = shape_document(&backend, &font, text).unwrap();
    let layout = wrap_document(&doc, 50.0); // line0 [0,6), line1 [6,8)

    assert_eq!(layout.line_for_byte(0), 0);
    assert_eq!(layout.line_for_byte(5), 0);
    // Byte 6 is the end of line 0 AND start of line 1 → resolves to line 1.
    assert_eq!(layout.line_for_byte(6), 1);
    // Document end resolves to the final line.
    assert_eq!(layout.line_for_byte(8), 1);
}

#[test]
fn caret_position_maps_byte_to_line_x_y() {
    let backend = MockBackend;
    let font = MockBackend::font();
    let text = "aa bb cc"; // each char 10 lpx, space 5
    let doc = shape_document(&backend, &font, text).unwrap();
    let layout = wrap_document(&doc, 50.0); // line0 "aa bb" [0,6), line1 "cc" [6,8)

    // Start of doc: line 0, x 0, y 0.
    assert_eq!(layout.caret_position(0), (0, 0.0, 0.0));
    // After "aa" (2 glyphs * 10): line 0, x 20.
    assert_eq!(layout.caret_position(2), (0, 20.0, 0.0));
    // Byte 6 is the wrap boundary → line 1 head (x 0, y = line_height).
    assert_eq!(layout.caret_position(6), (1, 0.0, LINE_HEIGHT));
    // Document end: line 1, x = "cc" width (20).
    assert_eq!(layout.caret_position(8), (1, 20.0, LINE_HEIGHT));
}
