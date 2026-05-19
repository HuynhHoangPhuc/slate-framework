//! Shared mock types for slate-text integration tests.
//!
//! Provides `MockFont` and `MockBackend` for tests that need a predictable,
//! GPU-independent text backend. Tests with specialised shaping or rasterization
//! logic define their own inline backend variants.

#![allow(dead_code)]

use slate_text::Font;
use slate_text::TextBackend;
use slate_text::error::TextError;
use slate_text::font_handle::FontHandle;
use slate_text::types::{
    FontDescriptor, FontId, FontMetrics, GlyphBitmap, GlyphBounds, ShapedGlyph, ShapedLine,
};

// ── MockFont ─────────────────────────────────────────────────────────────────

/// A minimal font implementation backed by a raw pointer handle.
///
/// Used wherever a `Font` is required but real platform shaping is not needed.
pub struct MockFont {
    pub handle: FontHandle,
    pub size_lpx: f32,
    pub scale: f32,
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

// ── MockBackend ───────────────────────────────────────────────────────────────

/// A predictable `TextBackend` that produces size-based bitmaps.
///
/// - `shape_line`: each character advances `x_advance_lpx` logical pixels.
/// - `rasterize_glyph`: `width = glyph_id.clamp(1, 16)`, `height = variant + 1`.
///
/// Tests that require different shaping or rasterization logic should define a
/// local `MockBackend` rather than using this one.
pub struct MockBackend {
    /// Advance width per character returned by `shape_line`.
    pub x_advance_lpx: f32,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self {
            x_advance_lpx: 10.0,
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
        Ok(MockFont {
            handle: FontHandle::from_face_id(0x12345678, size_lpx, scale),
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
            handle: FontHandle::from_face_id(bytes.as_ptr() as u64, size_lpx, scale),
            size_lpx,
            scale,
        })
    }

    fn shape_line(&self, _font: &Self::Font, text: &str) -> Result<ShapedLine, TextError> {
        let advance = self.x_advance_lpx;
        let glyphs: Vec<ShapedGlyph> = text
            .chars()
            .enumerate()
            .map(|(i, _)| ShapedGlyph {
                glyph_id: i as u32 + 1, // start at 1 to avoid 0-size bitmaps
                font_id: FontId::PRIMARY,
                font_handle: Default::default(),
                x_advance_lpx: advance,
                x_offset_lpx: 0.0,
                y_offset_lpx: 0.0,
                cluster: 0,
            })
            .collect();
        let width = glyphs.len() as f32 * advance;
        Ok(ShapedLine {
            glyphs,
            width_lpx: width,
            ascent_lpx: 12.0,
            descent_lpx: -3.0,
            y_offset_lpx: 0.0,
        })
    }

    fn rasterize_glyph(
        &self,
        _font: &Self::Font,
        glyph_id: u32,
        variant: u8,
    ) -> Result<GlyphBitmap, TextError> {
        // Predictable bitmap: width = glyph_id clamped to [1,16], height = variant+1
        let w = glyph_id.clamp(1, 16);
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

    fn glyph_raster_bounds(
        &self,
        _font: &Self::Font,
        glyph_id: u32,
    ) -> Result<GlyphBounds, TextError> {
        let w = glyph_id.clamp(1, 16);
        Ok(GlyphBounds {
            width: w,
            height: 4,
        })
    }

    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError> {
        Ok(vec![])
    }
}
