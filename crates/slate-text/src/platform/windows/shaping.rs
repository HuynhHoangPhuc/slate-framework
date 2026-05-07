//! Text shaping via IDWriteTextLayout with custom IDWriteTextRenderer.
//!
//! Uses the full DirectWrite shaping pipeline for kerned, GPOS-adjusted glyph advances.

use crate::types::FontId;
use crate::{FontMetrics, ShapedGlyph, ShapedLine, TextError};
use std::cell::RefCell;
use std::sync::Arc;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_GLYPH_RUN, DWRITE_GLYPH_RUN_DESCRIPTION, DWRITE_MATRIX, DWRITE_MEASURING_MODE,
    DWRITE_STRIKETHROUGH, DWRITE_UNDERLINE, IDWriteFactory5, IDWriteInlineObject,
    IDWritePixelSnapping_Impl, IDWriteTextFormat, IDWriteTextRenderer, IDWriteTextRenderer_Impl,
};
use windows::core::{BOOL, IUnknown, Ref, Result, implement};

/// Shared glyph storage for renderer callback.
type GlyphStore = Arc<RefCell<Vec<ShapedGlyph>>>;

/// Custom text renderer that collects shaped glyphs from DrawGlyphRun callbacks.
#[implement(IDWriteTextRenderer)]
pub(crate) struct ShapingRenderer {
    glyphs: GlyphStore,
}

impl ShapingRenderer {
    /// Create a new renderer with shared glyph storage.
    pub(crate) fn new(glyphs: GlyphStore) -> Self {
        Self { glyphs }
    }
}

#[allow(non_snake_case)]
impl IDWritePixelSnapping_Impl for ShapingRenderer_Impl {
    fn IsPixelSnappingDisabled(
        &self,
        _clientdrawingcontext: *const core::ffi::c_void,
    ) -> Result<BOOL> {
        Ok(BOOL(0)) // false - pixel snapping enabled
    }

    fn GetCurrentTransform(
        &self,
        _clientdrawingcontext: *const core::ffi::c_void,
        transform: *mut DWRITE_MATRIX,
    ) -> Result<()> {
        unsafe {
            *transform = DWRITE_MATRIX {
                m11: 1.0,
                m12: 0.0,
                m21: 0.0,
                m22: 1.0,
                dx: 0.0,
                dy: 0.0,
            };
        }
        Ok(())
    }

    fn GetPixelsPerDip(&self, _clientdrawingcontext: *const core::ffi::c_void) -> Result<f32> {
        Ok(1.0)
    }
}

#[allow(non_snake_case)]
impl IDWriteTextRenderer_Impl for ShapingRenderer_Impl {
    fn DrawGlyphRun(
        &self,
        _clientdrawingcontext: *const core::ffi::c_void,
        _baselineoriginx: f32,
        _baselineoriginy: f32,
        _measuringmode: DWRITE_MEASURING_MODE,
        glyphrun: *const DWRITE_GLYPH_RUN,
        _glyphrundescription: *const DWRITE_GLYPH_RUN_DESCRIPTION,
        _clientdrawingeffect: Ref<'_, IUnknown>,
    ) -> Result<()> {
        let run = unsafe { &*glyphrun };
        let count = run.glyphCount as usize;

        if count == 0 {
            return Ok(());
        }

        let indices = unsafe { std::slice::from_raw_parts(run.glyphIndices, count) };
        let advances = unsafe { std::slice::from_raw_parts(run.glyphAdvances, count) };
        let offsets = if run.glyphOffsets.is_null() {
            None
        } else {
            Some(unsafe { std::slice::from_raw_parts(run.glyphOffsets, count) })
        };

        let mut glyphs = self.glyphs.borrow_mut();
        glyphs.reserve(count);

        for i in 0..count {
            glyphs.push(ShapedGlyph {
                glyph_id: indices[i] as u32,
                font_id: FontId::PRIMARY,
                x_advance_lpx: advances[i],
                x_offset_lpx: offsets.map_or(0.0, |o| o[i].advanceOffset),
                y_offset_lpx: offsets.map_or(0.0, |o| o[i].ascenderOffset),
            });
        }

        Ok(())
    }

    fn DrawUnderline(
        &self,
        _clientdrawingcontext: *const core::ffi::c_void,
        _baselineoriginx: f32,
        _baselineoriginy: f32,
        _underline: *const DWRITE_UNDERLINE,
        _clientdrawingeffect: Ref<'_, IUnknown>,
    ) -> Result<()> {
        Ok(()) // no-op
    }

    fn DrawStrikethrough(
        &self,
        _clientdrawingcontext: *const core::ffi::c_void,
        _baselineoriginx: f32,
        _baselineoriginy: f32,
        _strikethrough: *const DWRITE_STRIKETHROUGH,
        _clientdrawingeffect: Ref<'_, IUnknown>,
    ) -> Result<()> {
        Ok(()) // no-op
    }

    fn DrawInlineObject(
        &self,
        _clientdrawingcontext: *const core::ffi::c_void,
        _originx: f32,
        _originy: f32,
        _inlineobject: Ref<'_, IDWriteInlineObject>,
        _issideways: BOOL,
        _isrighttoleft: BOOL,
        _clientdrawingeffect: Ref<'_, IUnknown>,
    ) -> Result<()> {
        Ok(()) // no-op
    }
}

/// Shape a line of text using the full DirectWrite shaping pipeline.
pub fn shape_line(
    factory: &IDWriteFactory5,
    text_format: &IDWriteTextFormat,
    text: &str,
    metrics: &FontMetrics,
) -> std::result::Result<ShapedLine, TextError> {
    if text.is_empty() {
        return Ok(ShapedLine {
            glyphs: vec![],
            width_lpx: 0.0,
            ascent_lpx: metrics.ascent_lpx,
            descent_lpx: metrics.descent_lpx,
            y_offset_lpx: 0.0,
        });
    }

    // UTF-8 → UTF-16
    let wide: Vec<u16> = text.encode_utf16().collect();

    // CreateTextLayout takes &[u16]
    let layout = unsafe { factory.CreateTextLayout(&wide, text_format, f32::MAX, f32::MAX) }
        .map_err(|e| TextError::ShapingFailed(format!("CreateTextLayout: {e}")))?;

    // Arc<RefCell> shared state for glyph accumulation
    let glyphs_store: GlyphStore = Arc::new(RefCell::new(Vec::new()));
    let renderer = ShapingRenderer::new(Arc::clone(&glyphs_store));
    let renderer_iface: IDWriteTextRenderer = renderer.into();

    // Draw() invokes DrawGlyphRun callback(s)
    unsafe { layout.Draw(None, &renderer_iface, 0.0, 0.0) }
        .map_err(|e| TextError::ShapingFailed(format!("IDWriteTextLayout::Draw: {e}")))?;

    let glyphs: Vec<ShapedGlyph> = glyphs_store.borrow_mut().drain(..).collect();
    let width_lpx = glyphs.iter().map(|g| g.x_advance_lpx).sum();

    Ok(ShapedLine {
        glyphs,
        width_lpx,
        ascent_lpx: metrics.ascent_lpx,
        descent_lpx: metrics.descent_lpx,
        y_offset_lpx: 0.0,
    })
}
