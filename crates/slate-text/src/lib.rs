//! Native text shaping and rasterization for Slate.
//!
//! This crate provides platform-native text rendering via CoreText (macOS) and
//! DirectWrite (Windows), feeding glyphs into slate-renderer's GlyphPipeline.
//!
//! # Phase 2a Scope
//!
//! - Single-line LTR text
//! - Single font (bundled DejaVu Sans)
//! - 4 sub-pixel X variants for glyph positioning
//!
//! Multi-line, wrap, BiDi, font fallback, and themed colors are deferred to Phase 2b.
//!
//! # Unit Convention
//!
//! All measurements use **logical pixels (lpx)** as the canonical unit:
//! - 1 lpx = 1 DIP at scale=1.0 = 1 point × 96/72
//! - CoreText (point-based) converts at font load
//! - DirectWrite (DIP-native) is 1:1 with logical pixels

pub mod backend;
pub mod error;
pub mod font_handle;
pub mod glyph_cache;
pub mod platform;
pub mod run_builder;
pub mod types;

pub use backend::{Font, TextBackend};
pub use error::TextError;
pub use font_handle::FontHandle;
pub use glyph_cache::GlyphCache;
pub use run_builder::TextRunBuilder;
pub use types::{CachedGlyph, FontMetrics, GlyphBitmap, GlyphMetrics, ShapedGlyph, ShapedLine};

/// Bundled DejaVu Sans font for testing and default text rendering.
pub const TEST_FONT: &[u8] = include_bytes!("../assets/dejavu-sans.ttf");

/// DejaVu Sans license text (Bitstream Vera license).
/// Binary distributions must surface this attribution.
pub const DEJAVU_SANS_LICENSE: &str = include_str!("../assets/LICENSE-DejaVu");

#[cfg(target_os = "windows")]
pub use platform::windows::DirectWriteBackend;

#[cfg(target_os = "macos")]
pub use platform::macos::CoreTextBackend;
