//! DirectWrite text backend for Windows.
//!
//! Provides native text shaping and rasterization using the Windows DirectWrite API.
//! Uses IDWriteInMemoryFontFileLoader (DirectWrite 5+) to load fonts from static byte slices.

mod font_id;
mod font_load;
mod rasterize;
mod shaping;
mod system_fonts;

use crate::{
    FontHandle, FontMetrics, GlyphBitmap, GlyphBounds, ShapedLine, TextBackend, TextError,
    backend::Font, types::FontDescriptor,
};
use std::cell::RefCell;
use std::collections::HashMap;
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
    /// `FontHandle → DirectWriteFont` registry populated by `shape_line` as
    /// it observes substitute faces chosen by DirectWrite's system fallback.
    /// Backed by `RefCell` so it can grow during the `&self`-borrowed
    /// `shape_line` call; downstream rasterize reads via `font_for`.
    font_registry: RefCell<HashMap<FontHandle, Box<DirectWriteFont>>>,
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
            font_registry: RefCell::new(HashMap::new()),
            _not_send: PhantomData,
        })
    }

    /// Build a minimal `DirectWriteFont` from a captured substitute face.
    ///
    /// Metrics are derived from the substitute face at the line's `size_lpx`.
    /// `text_format` is `None` because the substitute is only used for
    /// rasterize + bounds queries, never for further shaping.
    fn build_substitute_font(
        face: IDWriteFontFace,
        size_lpx: f32,
        scale: f32,
        handle: FontHandle,
    ) -> DirectWriteFont {
        let metrics = font_load::extract_metrics(&face, size_lpx);
        DirectWriteFont {
            font_face: face,
            em_size_dip: size_lpx,
            pixels_per_dip: scale,
            size_lpx,
            scale,
            metrics,
            text_format: None,
            handle,
        }
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
///
/// `text_format` is `None` for substitute fonts captured during shaping and
/// stored in the backend registry — those fonts are only used for rasterize +
/// bounds queries, which need `font_face` and sizing but never `text_format`.
pub struct DirectWriteFont {
    pub(crate) font_face: IDWriteFontFace,
    pub(crate) em_size_dip: f32,
    pub(crate) pixels_per_dip: f32,
    pub(crate) size_lpx: f32,
    pub(crate) scale: f32,
    pub(crate) metrics: FontMetrics,
    pub(crate) text_format: Option<IDWriteTextFormat>,
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

    fn load_font(
        &mut self,
        family: &str,
        size_lpx: f32,
        scale: f32,
    ) -> Result<Self::Font, TextError> {
        font_load::load_system_font(&self.factory, family, size_lpx, scale)
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
        let text_format = font.text_format.as_ref().ok_or_else(|| {
            TextError::ShapingFailed(
                "DirectWriteFont has no IDWriteTextFormat (substitute-only)".into(),
            )
        })?;
        let result = shaping::shape_line(
            &self.factory,
            text_format,
            text,
            &font.metrics,
            font.size_lpx,
            font.scale,
            font.handle,
        )?;
        // Register every substitute face captured during this Draw() so
        // per-glyph rasterize dispatch (via FontHandle) can resolve them.
        if !result.captured_faces.is_empty() {
            let mut reg = self.font_registry.borrow_mut();
            for cf in result.captured_faces {
                reg.entry(cf.handle).or_insert_with(|| {
                    Box::new(Self::build_substitute_font(
                        cf.face,
                        font.size_lpx,
                        font.scale,
                        cf.handle,
                    ))
                });
            }
        }
        Ok(result.line)
    }

    fn font_for(&self, handle: FontHandle) -> Option<&Self::Font> {
        // SAFETY rests on three invariants — any future change that breaks one
        // MUST update this comment and the parallel macOS impl:
        //   INVARIANT (append-only): `font_registry` is only ever inserted into,
        //     never removed/cleared/drained. Adding eviction silently breaks the
        //     pointer-extension below — the heap `DirectWriteFont` could be
        //     freed while we still hold `&*ptr`.
        //   INVARIANT (heap stability): values are `Box<DirectWriteFont>`. The
        //     `Box` itself moves on `HashMap` resize, but the heap allocation
        //     it owns does not.
        //   INVARIANT (single-thread): backend is `!Send` (PhantomData<*const ()>),
        //     so no thread can mutate the map while we hold the raw pointer.
        // The RefCell borrow is dropped before returning so callers can issue
        // further `&self` calls on the backend without a `BorrowMutError`.
        let map = self.font_registry.borrow();
        let ptr: *const DirectWriteFont = map.get(&handle).map(|b| &**b as *const _)?;
        drop(map);
        Some(unsafe { &*ptr })
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
