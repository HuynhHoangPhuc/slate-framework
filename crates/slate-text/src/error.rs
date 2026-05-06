//! Text rendering error types.

use thiserror::Error;

/// Errors that can occur during text shaping and rasterization.
#[derive(Debug, Error)]
pub enum TextError {
    /// Font family not found in system fonts.
    #[error("font not found: family `{family}`")]
    FontNotFound {
        /// The requested font family name.
        family: String,
    },

    /// Glyph ID not present in the font.
    #[error("glyph id {glyph_id} not in font")]
    GlyphNotFound {
        /// The missing glyph ID.
        glyph_id: u32,
    },

    /// Text shaping operation failed.
    #[error("shaping failed: {0}")]
    ShapingFailed(String),

    /// Glyph rasterization failed.
    #[error("rasterization failed: {0}")]
    RasterizationFailed(String),

    /// Font file could not be loaded.
    #[error("font file load failed: {0}")]
    FontFileLoad(String),

    /// Backend initialization failed.
    #[error("backend init failed: {0}")]
    BackendInit(String),

    /// Glyph variant not cached — caller forgot to materialize+flush before building instances.
    #[error("glyph {glyph_id} variant {variant} not cached")]
    GlyphNotCached {
        /// The missing glyph ID.
        glyph_id: u32,
        /// The sub-pixel variant (0-3).
        variant: u8,
    },

    /// System font enumeration failed.
    #[error("system font enumeration failed: {0}")]
    SystemFontEnumeration(String),
}
