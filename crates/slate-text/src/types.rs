//! Text rendering type vocabulary.
//!
//! All `*_lpx` fields use **logical pixels** as the canonical unit:
//! - 1 lpx = 1 DIP at scale=1.0 = 1 point × 96/72
//! - CoreText (point-based) converts at `load_font`
//! - DirectWrite (DIP-native) is 1:1

/// Result of shaping a single line of text.
#[derive(Clone, Debug)]
pub struct ShapedLine {
    /// Shaped glyphs in visual order.
    pub glyphs: Vec<ShapedGlyph>,
    /// Total advance width in logical pixels.
    pub width_lpx: f32,
    /// Ascent above baseline in logical pixels (positive).
    pub ascent_lpx: f32,
    /// Descent below baseline in logical pixels (negative).
    pub descent_lpx: f32,
}

/// A single shaped glyph with positioning info.
#[derive(Copy, Clone, Debug)]
pub struct ShapedGlyph {
    /// Glyph index in the font.
    pub glyph_id: u32,
    /// Horizontal advance to next glyph in logical pixels.
    pub x_advance_lpx: f32,
    /// Horizontal offset from pen position in logical pixels.
    pub x_offset_lpx: f32,
    /// Vertical offset from baseline in logical pixels.
    pub y_offset_lpx: f32,
}

/// Rasterized glyph bitmap with metrics.
#[derive(Clone, Debug)]
pub struct GlyphBitmap {
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels.
    pub height: u32,
    /// Horizontal bearing from pen to left edge in logical pixels.
    pub bearing_x_lpx: f32,
    /// Vertical bearing from baseline to top edge in logical pixels (positive-up).
    pub bearing_y_lpx: f32,
    /// Horizontal advance in logical pixels.
    pub advance_x_lpx: f32,
    /// Alpha channel data, R8 row-major, no padding.
    pub alpha: Vec<u8>,
}

/// Font-level metrics in logical pixels.
#[derive(Copy, Clone, Debug)]
pub struct FontMetrics {
    /// Distance from baseline to top of tallest glyph (positive).
    pub ascent_lpx: f32,
    /// Distance from baseline to bottom of lowest glyph (negative).
    pub descent_lpx: f32,
    /// Recommended additional spacing between lines.
    pub line_gap_lpx: f32,
    /// Height of lowercase 'x' character.
    pub x_height_lpx: f32,
    /// Height of uppercase letters.
    pub cap_height_lpx: f32,
    /// Font design units per em.
    pub units_per_em: u32,
}

/// Metrics for a cached glyph, stored alongside atlas allocation.
#[derive(Copy, Clone, Debug)]
pub struct GlyphMetrics {
    /// Bitmap width in pixels.
    pub width: u32,
    /// Bitmap height in pixels.
    pub height: u32,
    /// Horizontal bearing in logical pixels.
    pub bearing_x_lpx: f32,
    /// Vertical bearing in logical pixels.
    pub bearing_y_lpx: f32,
    /// Horizontal advance in logical pixels.
    pub advance_x_lpx: f32,
}

/// A glyph that has been rasterized and allocated in the atlas.
#[derive(Copy, Clone, Debug)]
pub struct CachedGlyph {
    /// Atlas allocation handle.
    pub alloc: slate_renderer::atlas::AtlasAllocation,
    /// Glyph metrics.
    pub metrics: GlyphMetrics,
}
