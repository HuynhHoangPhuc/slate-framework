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
use std::cell::RefCell;
use std::collections::HashMap;
use std::marker::PhantomData;

/// CoreText text backend.
///
/// Marked `!Send + !Sync` for API parity with DirectWrite, though CoreText is thread-safe.
pub struct CoreTextBackend {
    /// `FontHandle → CoreTextFont` registry populated by `shape_line` as it
    /// observes the per-`CTRun` substitute font CoreText chose for missing
    /// codepoints. Backed by `RefCell` so it can grow during the
    /// `&self`-borrowed `shape_line`; downstream rasterize reads via
    /// `font_for`.
    font_registry: RefCell<HashMap<FontHandle, Box<CoreTextFont>>>,
    _not_send: PhantomData<*const ()>,
}

impl CoreTextBackend {
    /// Create a new CoreText backend.
    ///
    /// Returns Ok on macOS since CoreText is always available.
    /// Returns Result for API parity with DirectWriteBackend.
    pub fn new() -> Result<Self, TextError> {
        Ok(Self {
            font_registry: RefCell::new(HashMap::new()),
            _not_send: PhantomData,
        })
    }

    /// Build a minimal `CoreTextFont` from a substitute `CTFont` captured
    /// during shaping. Metrics are derived from the substitute as CoreText
    /// returned it; per Apple's docs the cascade propagates the parent
    /// attribute's point size, so the substitute should already be at
    /// `size_lpx`. The `debug_assert!` catches a future regression where a
    /// cascade font comes back at a normalized 12pt baseline. No
    /// `_data_retain` because the substitute came from CoreText's system
    /// cascade, not from `load_font_from_bytes`.
    fn build_substitute_font(
        ct_font: CFRetained<CTFont>,
        size_lpx: f32,
        scale: f32,
        handle: FontHandle,
    ) -> CoreTextFont {
        debug_assert!(
            {
                let actual_pt = unsafe { ct_font.size() } as f32;
                (actual_pt - size_lpx).abs() < 0.5
            },
            "substitute CTFont size disagrees with parent line size_lpx={size_lpx}",
        );
        let metrics = font_load::extract_metrics(&ct_font);
        CoreTextFont {
            ct_font,
            _data_retain: None,
            size_lpx,
            scale,
            metrics,
            handle,
        }
    }
}

impl Default for CoreTextBackend {
    fn default() -> Self {
        Self::new().expect("CoreText backend initialization should never fail")
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
        let result =
            shaping::shape_line(&font.ct_font, text, &font.metrics, font.size_lpx, font.scale)?;
        if !result.captured_fonts.is_empty() {
            let mut reg = self.font_registry.borrow_mut();
            for cf in result.captured_fonts {
                reg.entry(cf.handle).or_insert_with(|| {
                    Box::new(Self::build_substitute_font(
                        cf.font,
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
        // SAFETY: see `windows/mod.rs::font_for` for the full invariant chain.
        // Same three preconditions apply here:
        //   INVARIANT (append-only): `font_registry` never has entries removed.
        //   INVARIANT (heap stability): `Box<CoreTextFont>` heap addresses are
        //     stable across `HashMap` resizes.
        //   INVARIANT (single-thread): backend is `!Send` (PhantomData<*const ()>).
        let map = self.font_registry.borrow();
        let ptr: *const CoreTextFont = map.get(&handle).map(|b| &**b as *const _)?;
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
            &font.ct_font,
            glyph_id_u16,
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
        let glyph_id_u16 =
            u16::try_from(glyph_id).map_err(|_| TextError::GlyphNotFound { glyph_id })?;
        rasterize::get_glyph_bounds(&font.ct_font, glyph_id_u16, font.scale)
    }

    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError> {
        system_fonts::enumerate_system_fonts()
    }
}
