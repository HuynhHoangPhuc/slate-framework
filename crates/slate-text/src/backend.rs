//! Text backend trait for native shaping and rasterization.
//!
//! # Unit Convention
//!
//! All measurements use **logical pixels (lpx)** as the canonical unit:
//! - 1 lpx = 1 DIP at scale=1.0 = 1 point × 96/72
//! - CoreText (point-based) converts at `load_font`: `size_pt → size_lpx = size_pt * 96/72`
//! - DirectWrite (DIP-native) is 1:1 with logical pixels
//!
//! # Sub-pixel Variants
//!
//! The `variant` parameter in `rasterize_glyph` ranges from 0 to 3, representing
//! sub-pixel X offsets for improved glyph positioning:
//! - `variant 0`: no offset
//! - `variant 1`: 0.25 pixel offset
//! - `variant 2`: 0.5 pixel offset
//! - `variant 3`: 0.75 pixel offset
//!
//! # Thread Safety
//!
//! `TextBackend` and `Font` are NOT required to be `Send` or `Sync`.
//! DirectWrite is apartment-threaded (`!Send` on `IDWriteFactory`).
//! CoreText happens to be thread-safe, but we keep parity across platforms.

use crate::error::TextError;
use crate::font_handle::FontHandle;
use crate::paragraph::greedy_wrap;
use crate::types::{FontDescriptor, FontMetrics, GlyphBitmap, GlyphBounds, ShapedLine};

/// Native text shaping and rasterization backend.
///
/// Implementations provide platform-specific text rendering via CoreText (macOS)
/// or DirectWrite (Windows). The trait is NOT object-safe due to the associated
/// `Font` type.
///
/// # Thread Safety
///
/// Implementations are NOT required to be `Send` or `Sync`. DirectWrite uses
/// apartment threading; use a single backend instance per thread.
pub trait TextBackend {
    /// Platform-specific font type.
    type Font: Font;

    /// Load a font by family name from system fonts.
    ///
    /// # Arguments
    ///
    /// * `family` - Font family name (e.g., "Helvetica", "Segoe UI")
    /// * `size_lpx` - Font size in logical pixels
    /// * `scale` - Display scale factor (e.g., 2.0 for Retina)
    ///
    /// # Phase 2a Note
    ///
    /// System font lookup is deferred to Phase 2b. Implementations may return
    /// `TextError::FontNotFound` for all family lookups in Phase 2a.
    fn load_font(
        &mut self,
        family: &str,
        size_lpx: f32,
        scale: f32,
    ) -> Result<Self::Font, TextError>;

    /// Load a font from raw TTF/OTF bytes.
    ///
    /// This is the primary path for Phase 2a using the bundled DejaVu Sans font.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Raw font file data (TTF/OTF format)
    /// * `size_lpx` - Font size in logical pixels
    /// * `scale` - Display scale factor
    fn load_font_from_bytes(
        &mut self,
        bytes: &'static [u8],
        size_lpx: f32,
        scale: f32,
    ) -> Result<Self::Font, TextError>;

    /// Shape a line of text into positioned glyphs.
    ///
    /// Returns glyph IDs with advance/offset information for rendering.
    /// Phase 2a supports single-line LTR text only.
    fn shape_line(&self, font: &Self::Font, text: &str) -> Result<ShapedLine, TextError>;

    /// Rasterize a glyph to an alpha bitmap.
    ///
    /// # Arguments
    ///
    /// * `font` - Font to rasterize from
    /// * `glyph_id` - Glyph index in the font
    /// * `variant` - Sub-pixel X variant (0-3, offset = variant/4 pixels)
    fn rasterize_glyph(
        &self,
        font: &Self::Font,
        glyph_id: u32,
        variant: u8,
    ) -> Result<GlyphBitmap, TextError>;

    /// Query glyph raster bounds without rasterizing.
    ///
    /// Returns `GlyphBounds::ZERO` for whitespace glyphs. This is the
    /// bounds-check-before-rasterize pattern: query bounds first (cheap, O(1) cached),
    /// then only rasterize if non-zero.
    ///
    /// # Arguments
    ///
    /// * `font` - Font to query
    /// * `glyph_id` - Glyph index in the font
    fn glyph_raster_bounds(
        &self,
        font: &Self::Font,
        glyph_id: u32,
    ) -> Result<GlyphBounds, TextError>;

    /// Shape a paragraph of text into multiple wrapped lines.
    ///
    /// Breaks text at spaces using greedy first-fit algorithm. Each returned
    /// `ShapedLine` has `y_offset_lpx` set to its vertical position.
    ///
    /// # Arguments
    ///
    /// * `font` - Font to use for shaping
    /// * `text` - Input text to wrap
    /// * `max_width_lpx` - Maximum line width in logical pixels
    fn shape_paragraph(
        &self,
        font: &Self::Font,
        text: &str,
        max_width_lpx: f32,
    ) -> Result<Vec<ShapedLine>, TextError>
    where
        Self: Sized,
    {
        greedy_wrap(self, font, text, max_width_lpx)
    }

    /// Enumerate system fonts without loading them.
    ///
    /// Returns metadata for all installed fonts. This is O(1) per font with
    /// no font file loading—only metadata extraction.
    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError>;
}

/// Platform font instance with metrics and handle.
///
/// # Thread Safety
///
/// NOT required to be `Send` or `Sync`. See `TextBackend` documentation.
///
/// # Handle Uniqueness
///
/// `handle()` returns a `FontHandle` combining pointer address, size, and scale.
/// Pointer alone is insufficient because platforms may recycle pointers across
/// font reloads at different sizes.
pub trait Font {
    /// Returns the unique handle for cache keying.
    fn handle(&self) -> FontHandle;

    /// Returns font-level metrics (ascent, descent, etc.).
    fn metrics(&self) -> FontMetrics;

    /// Returns the font size in logical pixels.
    fn size_lpx(&self) -> f32;

    /// Returns the display scale factor.
    fn scale(&self) -> f32;
}
