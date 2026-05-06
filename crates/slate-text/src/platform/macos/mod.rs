//! CoreText text backend for macOS.
//!
//! Provides native text shaping and rasterization using CoreText and CoreGraphics.
//! Uses CTFontManagerCreateFontDescriptorFromData to load fonts from static byte slices.

mod font_load;
mod rasterize;
mod shaping;
mod system_fonts;

use crate::{
    FontHandle, FontMetrics, GlyphBitmap, GlyphBounds, ShapedLine, TextBackend, TextError,
    backend::Font, types::FontDescriptor,
};
use objc2_core_foundation::{CFData, CFRetained};
use objc2_core_text::CTFont;
use std::marker::PhantomData;

/// CoreText points to logical pixels conversion factor.
/// 1 lpx = 1/96 inch, 1 pt = 1/72 inch, so lpx = pt * 96/72.
pub(crate) const PT_TO_LPX: f32 = 96.0 / 72.0;

/// CoreText text backend.
///
/// Marked `!Send + !Sync` for API parity with DirectWrite, though CoreText is thread-safe.
pub struct CoreTextBackend {
    _not_send: PhantomData<*const ()>,
}

impl CoreTextBackend {
    /// Create a new CoreText backend.
    pub fn new() -> Self {
        Self {
            _not_send: PhantomData,
        }
    }
}

impl Default for CoreTextBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// CoreText font handle.
///
/// Holds the CTFont, optional CFData (for fonts loaded from bytes), metrics,
/// and rendering parameters.
pub struct CoreTextFont {
    ct_font: CFRetained<CTFont>,
    /// Keeps the byte-backed CFData alive for fonts loaded from static slices.
    /// Must not be dropped before ct_font.
    _data_retain: Option<CFRetained<CFData>>,
    size_lpx: f32,
    scale: f32,
    metrics: FontMetrics,
    handle: FontHandle,
}

impl Font for CoreTextFont {
    fn handle(&self) -> FontHandle {
        self.handle
    }

    fn metrics(&self) -> FontMetrics {
        self.metrics
    }

    fn size_lpx(&self) -> f32 {
        self.size_lpx
    }

    fn scale(&self) -> f32 {
        self.scale
    }
}

impl TextBackend for CoreTextBackend {
    type Font = CoreTextFont;

    /// System-font lookup is deferred to Phase 2b.
    fn load_font(
        &mut self,
        family: &str,
        _size_lpx: f32,
        _scale: f32,
    ) -> Result<Self::Font, TextError> {
        Err(TextError::FontNotFound {
            family: family.to_string(),
        })
    }

    fn load_font_from_bytes(
        &mut self,
        bytes: &'static [u8],
        size_lpx: f32,
        scale: f32,
    ) -> Result<Self::Font, TextError> {
        let (ct_font, data) = font_load::create_font_from_bytes(bytes, size_lpx)?;
        let metrics = font_load::extract_metrics(&ct_font);

        // Build handle from font pointer + size + scale
        let ptr = CFRetained::as_ptr(&ct_font).as_ptr() as *const u8;
        let handle = FontHandle::from_ptr_size_scale(ptr, size_lpx, scale);

        Ok(CoreTextFont {
            ct_font,
            _data_retain: Some(data),
            size_lpx,
            scale,
            metrics,
            handle,
        })
    }

    fn shape_line(&self, font: &Self::Font, text: &str) -> Result<ShapedLine, TextError> {
        shaping::shape_line(&font.ct_font, text, &font.metrics)
    }

    fn rasterize_glyph(
        &self,
        font: &Self::Font,
        glyph_id: u32,
        variant: u8,
    ) -> Result<GlyphBitmap, TextError> {
        rasterize::rasterize(
            &font.ct_font,
            glyph_id as u16,
            font.size_lpx,
            font.scale,
            variant,
        )
    }

    fn glyph_raster_bounds(
        &self,
        font: &Self::Font,
        glyph_id: u32,
    ) -> Result<GlyphBounds, TextError> {
        rasterize::get_glyph_bounds(&font.ct_font, glyph_id as u16, font.scale)
    }

    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError> {
        system_fonts::enumerate_system_fonts()
    }
}
