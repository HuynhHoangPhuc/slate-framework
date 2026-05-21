//! Native text shaping and rasterization for Slate.
//!
//! This crate provides platform-native text rendering via CoreText (macOS) and
//! DirectWrite (Windows), feeding glyphs into slate-renderer's GlyphPipeline.
//!
//! # Phase 2b Capabilities
//!
//! - Single-line and multi-line text with greedy word wrap
//! - Font fallback chains with system font enumeration
//! - 4 sub-pixel X variants for glyph positioning
//! - Font smoothing dilation (macOS) using BT.709 luminance weights
//! - Line-layout cache with two-frame rolling eviction
//!
//! # Unit Convention
//!
//! All measurements use **logical pixels (lpx)** as the canonical unit:
//! - 1 lpx = 1 DIP at scale=1.0 = 1 point × 96/72
//! - CoreText (point-based) converts at font load
//! - DirectWrite (DIP-native) is 1:1 with logical pixels

pub mod backend;
pub(crate) mod bidi;
pub mod bounds_cache;
pub(crate) mod cluster;
pub mod deferred_font;
pub mod dilation;
pub mod error;
pub mod fallback;
pub mod font_handle;
pub mod glyph_cache;
pub mod glyph_geometry;
pub(crate) mod linebreak;
pub mod line_layout_cache;
pub mod multiline;
pub mod paragraph;
pub mod platform;
pub mod run_builder;
pub mod types;

pub use backend::{Font, TextBackend};
pub use bounds_cache::RasterBoundsCache;
pub use deferred_font::{CharacterSet, DeferredFont};
pub use dilation::{MAX_DILATION, compute_dilation, compute_dilation_srgb, dilate_bounds};
pub use error::TextError;
pub use fallback::{FontFallbackSystem, fix_missing_glyphs};
pub use font_handle::FontHandle;
pub use glyph_cache::GlyphCache;
pub use glyph_geometry::{byte_at_pixel_x, pixel_x_at_byte};
pub use line_layout_cache::{LineLayoutCache, hash_text};
pub use multiline::{
    MultilineLayout, ShapedDocument, ShapedParagraph, VisualLine, shape_document, wrap_document,
};
pub use paragraph::{
    ShapedWord, compute_alignment_offset, greedy_wrap, shape_words, truncate_with_ellipsis,
    wrap_shaped_words,
};
pub use run_builder::TextRunBuilder;
pub use types::{
    CachedGlyph, Direction, FontDescriptor, FontId, FontMetrics, FontStyle, GlyphBitmap,
    GlyphBounds, GlyphMetrics, RunSpan, ShapedGlyph, ShapedLine, TextAlignment,
};

/// Bundled DejaVu Sans font for testing and default text rendering.
pub const TEST_FONT: &[u8] = include_bytes!("../assets/dejavu-sans.ttf");

/// DejaVu Sans license text (Bitstream Vera license).
/// Binary distributions must surface this attribution.
pub const DEJAVU_SANS_LICENSE: &str = include_str!("../assets/LICENSE-DejaVu");

/// Bundled Noto Sans JP Regular (subset OTF, ~4.3 MB).
///
/// Ships as the safety-path CJK fallback when system-font loading is
/// unavailable or unsafe. Covers Hiragana, Katakana, and the JIS-1 Han
/// subset used by everyday Japanese text. Hangul and Pinyin Han glyphs
/// are out of scope for this bundle — defer to platform fallback or a
/// future regional bundle if needed.
pub const BUNDLED_CJK_JP_FONT: &[u8] = include_bytes!("../assets/NotoSansJP-Regular.otf");

/// Noto Sans JP license text (SIL Open Font License 1.1).
/// Binary distributions must surface this attribution.
pub const NOTO_SANS_JP_LICENSE: &str = include_str!("../assets/LICENSE-NotoSansJP");

#[cfg(target_os = "windows")]
pub use platform::windows::DirectWriteBackend;

// Re-export windows_core for #[implement] macro (required by windows-rs 0.62)
#[cfg(target_os = "windows")]
pub extern crate windows_core;

#[cfg(target_os = "macos")]
pub use platform::macos::CoreTextBackend;
