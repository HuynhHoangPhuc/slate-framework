//! DirectWrite text backend for Windows.
//!
//! Provides native text shaping and rasterization using the Windows DirectWrite API.
//! Uses IDWriteInMemoryFontFileLoader (DirectWrite 5+) to load fonts from static byte slices.

mod font_load;
mod rasterize;
mod shaping;
mod system_fonts;

use crate::{
    FontHandle, FontMetrics, GlyphBitmap, GlyphBounds, ShapedLine, TextBackend, TextError,
    backend::Font, types::FontDescriptor,
};
use std::marker::PhantomData;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWriteCreateFactory, IDWriteFactory, IDWriteFactory5,
    IDWriteFontFace, IDWriteInMemoryFontFileLoader, IDWriteTextFormat,
};
use windows::core::Interface;

/// DirectWrite text backend.
///
/// Marked `!Send + !Sync` because DirectWrite factory and font faces are
/// apartment-threaded COM objects.
pub struct DirectWriteBackend {
    factory: IDWriteFactory5,
    loader: IDWriteInMemoryFontFileLoader,
    _not_send: PhantomData<*const ()>,
}

impl DirectWriteBackend {
    /// Create a new DirectWrite backend.
    ///
    /// Initializes the shared DirectWrite factory (v5+) and creates an in-memory
    /// font file loader for loading fonts from static byte slices.
    pub fn new() -> Result<Self, TextError> {
        // Create base factory then query for IDWriteFactory5
        let factory_base: IDWriteFactory =
            unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
                .map_err(|e| TextError::BackendInit(format!("DWriteCreateFactory: {}", e)))?;

        let factory: IDWriteFactory5 = factory_base
            .cast()
            .map_err(|e| TextError::BackendInit(format!("Cast to IDWriteFactory5: {}", e)))?;

        // Create the built-in in-memory font file loader
        let loader: IDWriteInMemoryFontFileLoader = unsafe {
            factory.CreateInMemoryFontFileLoader()
        }
        .map_err(|e| TextError::BackendInit(format!("CreateInMemoryFontFileLoader: {}", e)))?;

        // Register the loader with the factory
        unsafe { factory.RegisterFontFileLoader(&loader) }
            .map_err(|e| TextError::BackendInit(format!("RegisterFontFileLoader: {}", e)))?;

        Ok(Self {
            factory,
            loader,
            _not_send: PhantomData,
        })
    }
}

impl Drop for DirectWriteBackend {
    fn drop(&mut self) {
        if let Err(e) = unsafe { self.factory.UnregisterFontFileLoader(&self.loader) } {
            log::warn!("DirectWrite font loader unregister failed: {e}");
        }
    }
}

/// DirectWrite font handle.
///
/// Holds the font face, metrics, and rendering parameters.
pub struct DirectWriteFont {
    pub(crate) font_face: IDWriteFontFace,
    pub(crate) em_size_dip: f32,
    pub(crate) pixels_per_dip: f32,
    pub(crate) size_lpx: f32,
    pub(crate) scale: f32,
    pub(crate) metrics: FontMetrics,
    pub(crate) text_format: IDWriteTextFormat,
    pub(crate) handle: FontHandle,
}

impl Font for DirectWriteFont {
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

impl TextBackend for DirectWriteBackend {
    type Font = DirectWriteFont;

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
        font_load::load_font_from_bytes(&self.factory, &self.loader, bytes, size_lpx, scale)
    }

    fn shape_line(&self, font: &Self::Font, text: &str) -> Result<ShapedLine, TextError> {
        shaping::shape_line(&self.factory, &font.text_format, text, &font.metrics)
    }

    fn rasterize_glyph(
        &self,
        font: &Self::Font,
        glyph_id: u32,
        variant: u8,
    ) -> Result<GlyphBitmap, TextError> {
        let glyph_id_u16 =
            u16::try_from(glyph_id).map_err(|_| TextError::GlyphNotFound { glyph_id })?;
        rasterize::rasterize(
            &self.factory,
            &font.font_face,
            font.em_size_dip,
            font.pixels_per_dip,
            glyph_id_u16,
            variant,
        )
    }

    fn glyph_raster_bounds(
        &self,
        font: &Self::Font,
        glyph_id: u32,
    ) -> Result<GlyphBounds, TextError> {
        let glyph_id_u16 =
            u16::try_from(glyph_id).map_err(|_| TextError::GlyphNotFound { glyph_id })?;
        rasterize::get_glyph_bounds(
            &self.factory,
            &font.font_face,
            font.em_size_dip,
            glyph_id_u16,
        )
    }

    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError> {
        system_fonts::enumerate_system_fonts(&self.factory)
    }
}
