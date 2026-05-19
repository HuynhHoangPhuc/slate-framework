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
    /// Vertical offset from paragraph origin in logical pixels.
    /// For single-line shaping, this is 0.0.
    pub y_offset_lpx: f32,
}

/// Text alignment for multi-line layout.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TextAlignment {
    /// Left-aligned (default for LTR text).
    #[default]
    Left,
    /// Center-aligned within container width.
    Center,
    /// Right-aligned.
    Right,
}

/// A single shaped glyph with positioning info.
#[derive(Copy, Clone, Debug)]
pub struct ShapedGlyph {
    /// Glyph index in the font.
    pub glyph_id: u32,
    /// Logical font identifier (used by `FontFallbackSystem` only — not the
    /// cache key). Per-glyph rasterization keys on `font_handle`.
    pub font_id: FontId,
    /// Cache key + backend registry key for the face the platform shaper
    /// actually used for this glyph. May differ from the primary font when the
    /// platform shaper substituted a fallback for missing codepoints.
    /// Sentinel `FontHandle::default()` means "use the primary font".
    pub font_handle: crate::FontHandle,
    /// Horizontal advance to next glyph in logical pixels.
    pub x_advance_lpx: f32,
    /// Absolute glyph position `[x, y]` in logical pixels, relative to the
    /// line origin (line origin = pen at baseline at the start of the line).
    ///
    /// `position_lpx[0]` is the X pen position the glyph should be painted at
    /// (cumulative — already includes all preceding advances and any per-glyph
    /// nudge from GPOS / combining marks). `position_lpx[1]` is the baseline-
    /// relative Y, typically 0.0 except for vertically-shifted glyphs.
    ///
    /// This matches CoreText's `CTRunGetPositions` semantic directly; the
    /// DirectWrite adapter accumulates per-glyph advances + `glyphOffsets` into
    /// the same absolute form. Renderers paint at
    /// `baseline_lpx + position_lpx` without maintaining a pen accumulator.
    pub position_lpx: [f32; 2],
    /// UTF-8 byte offset into the source string for the leading character of
    /// the cluster this glyph belongs to (HarfBuzz convention). Glyphs in the
    /// same cluster share a value. For both LTR and RTL runs the value is the
    /// smallest UTF-8 byte offset of any character in the cluster, so cluster
    /// values are monotonically non-decreasing in logical order (which equals
    /// visual order for LTR; reverse of visual order for RTL).
    pub cluster: u32,
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

/// Raster bounds for a glyph (pre-rasterization query).
///
/// Used for bounds-check-before-rasterize pattern: query bounds first (cheap),
/// then only rasterize if non-zero. Whitespace glyphs return zero bounds.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GlyphBounds {
    /// Bitmap width in pixels (0 for whitespace).
    pub width: u32,
    /// Bitmap height in pixels (0 for whitespace).
    pub height: u32,
}

/// Unique identifier for a font in a fallback chain.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct FontId(pub u32);

impl FontId {
    /// Primary font ID (always 0).
    pub const PRIMARY: Self = Self(0);
}

/// Font style variant.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontStyle {
    #[default]
    Regular,
    Italic,
    Bold,
    BoldItalic,
}

/// System font metadata without loading the full font.
#[derive(Clone, Debug)]
pub struct FontDescriptor {
    /// Font family name (e.g., "Arial", "Helvetica").
    pub family: String,
    /// PostScript name for unique font identification.
    pub postscript_name: String,
    /// Font weight (100-900, 400 = regular, 700 = bold).
    pub weight: u16,
    /// Font style variant.
    pub style: FontStyle,
    /// Path to the font file, if available.
    pub path: Option<std::path::PathBuf>,
}

impl GlyphBounds {
    /// Zero bounds for whitespace glyphs.
    pub const ZERO: Self = Self {
        width: 0,
        height: 0,
    };

    /// Returns true if this glyph is whitespace (zero bounds).
    #[inline]
    pub fn is_whitespace(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}
