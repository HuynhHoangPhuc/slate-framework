//! Text shaping via IDWriteTextLayout with custom IDWriteTextRenderer.
//!
//! Uses the full DirectWrite shaping pipeline for kerned, GPOS-adjusted glyph advances.

use crate::types::FontId;
use crate::{FontHandle, FontMetrics, ShapedGlyph, ShapedLine, TextError};
use std::cell::RefCell;
use std::rc::Rc;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_GLYPH_RUN, DWRITE_GLYPH_RUN_DESCRIPTION, DWRITE_MATRIX, DWRITE_MEASURING_MODE,
    DWRITE_STRIKETHROUGH, DWRITE_UNDERLINE, IDWriteFactory5, IDWriteFontFace, IDWriteInlineObject,
    IDWritePixelSnapping_Impl, IDWriteTextFormat, IDWriteTextRenderer, IDWriteTextRenderer_Impl,
};
use windows::core::{BOOL, IUnknown, Interface, Ref, Result, implement};

/// One face captured from a `DrawGlyphRun` callback, paired with the
/// `FontHandle` derived from its raw COM pointer + the line's size/scale.
///
/// Returned alongside glyphs so the backend can populate its
/// `FontHandle → Font` registry without exposing COM types to upper layers.
pub(crate) struct CapturedFace {
    pub(crate) handle: FontHandle,
    pub(crate) face: IDWriteFontFace,
}

/// Shared glyph storage for renderer callback.
type GlyphStore = Rc<RefCell<Vec<ShapedGlyph>>>;
type FaceStore = Rc<RefCell<Vec<CapturedFace>>>;

/// Custom text renderer that collects shaped glyphs from DrawGlyphRun callbacks.
///
/// Each callback also yields the substitute `IDWriteFontFace` chosen by
/// DirectWrite's system fallback; we derive a `FontHandle` from its pointer +
/// the line's size/scale so the downstream rasterizer can dispatch per-glyph.
#[implement(IDWriteTextRenderer)]
pub(crate) struct ShapingRenderer {
    glyphs: GlyphStore,
    faces: FaceStore,
    size_lpx: f32,
    scale: f32,
    /// `FontHandle` for the primary face. Substitute capture skips entries
    /// matching this so the backend registry doesn't store the primary as a
    /// substitute (mirrors macOS `shaping.rs` skip-primary guard).
    primary_handle: FontHandle,
}

impl ShapingRenderer {
    /// Create a new renderer with shared glyph + face storage.
    pub(crate) fn new(
        glyphs: GlyphStore,
        faces: FaceStore,
        size_lpx: f32,
        scale: f32,
        primary_handle: FontHandle,
    ) -> Self {
        Self {
            glyphs,
            faces,
            size_lpx,
            scale,
            primary_handle,
        }
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

        // `glyphrun.fontFace` is `ManuallyDrop<Option<IDWriteFontFace>>` borrowed
        // for the duration of the callback. Deref reaches the inner `Option`, then
        // `Option::clone` calls `IDWriteFontFace::Clone` (windows-rs auto-impl ->
        // `IUnknown::AddRef`), producing a new strong ref that outlives Draw().
        // Empty optional means DirectWrite did not provide a face — falls back
        // to the primary handle via the default sentinel.
        let face_opt: Option<IDWriteFontFace> = (*run.fontFace).clone();
        let font_handle = match face_opt.as_ref() {
            Some(face) => {
                let ptr = face.as_raw() as *const u8;
                FontHandle::from_ptr_size_scale(ptr, self.size_lpx, self.scale)
            }
            None => FontHandle::default(),
        };

        // Record the face for backend registry insertion (post-Draw). Skip
        // entries matching the primary — the primary path in `run_builder`
        // routes through `self.font` directly and never queries the registry,
        // so storing it just wastes an `extract_metrics` + heap alloc.
        // Dedup against already-captured substitutes is per-callback; cross-call
        // dedup happens at the HashMap level in `mod.rs` via `entry().or_insert_with`.
        if font_handle != self.primary_handle
            && let Some(face) = face_opt
        {
            let mut faces = self.faces.borrow_mut();
            if !faces.iter().any(|cf| cf.handle == font_handle) {
                faces.push(CapturedFace {
                    handle: font_handle,
                    face,
                });
            }
        }

        let mut glyphs = self.glyphs.borrow_mut();
        glyphs.reserve(count);

        for i in 0..count {
            glyphs.push(ShapedGlyph {
                glyph_id: indices[i] as u32,
                font_id: FontId::PRIMARY,
                font_handle,
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

/// Result of shaping a line: the glyphs plus any new substitute faces the
/// platform shaper chose. Caller registers the faces so per-glyph rasterize
/// dispatch (via `font_handle`) can resolve them later.
pub(crate) struct ShapeResult {
    pub(crate) line: ShapedLine,
    pub(crate) captured_faces: Vec<CapturedFace>,
}

/// Shape a line of text using the full DirectWrite shaping pipeline.
///
/// `size_lpx` / `scale` parametrize the `FontHandle`s recorded on each glyph
/// (and on each `CapturedFace`) so the downstream cache key matches the
/// primary font's handle convention.
pub(crate) fn shape_line(
    factory: &IDWriteFactory5,
    text_format: &IDWriteTextFormat,
    text: &str,
    metrics: &FontMetrics,
    size_lpx: f32,
    scale: f32,
    primary_handle: FontHandle,
) -> std::result::Result<ShapeResult, TextError> {
    if text.is_empty() {
        return Ok(ShapeResult {
            line: ShapedLine {
                glyphs: vec![],
                width_lpx: 0.0,
                ascent_lpx: metrics.ascent_lpx,
                descent_lpx: metrics.descent_lpx,
                y_offset_lpx: 0.0,
            },
            captured_faces: Vec::new(),
        });
    }

    // UTF-8 → UTF-16
    let wide: Vec<u16> = text.encode_utf16().collect();

    // CreateTextLayout takes &[u16]
    let layout = unsafe { factory.CreateTextLayout(&wide, text_format, f32::MAX, f32::MAX) }
        .map_err(|e| TextError::ShapingFailed(format!("CreateTextLayout: {e}")))?;

    // Rc<RefCell> shared state for glyph + face accumulation
    let glyphs_store: GlyphStore = Rc::new(RefCell::new(Vec::new()));
    let faces_store: FaceStore = Rc::new(RefCell::new(Vec::new()));
    let renderer = ShapingRenderer::new(
        Rc::clone(&glyphs_store),
        Rc::clone(&faces_store),
        size_lpx,
        scale,
        primary_handle,
    );
    let renderer_iface: IDWriteTextRenderer = renderer.into();

    // Draw() invokes DrawGlyphRun callback(s)
    unsafe { layout.Draw(None, &renderer_iface, 0.0, 0.0) }
        .map_err(|e| TextError::ShapingFailed(format!("IDWriteTextLayout::Draw: {e}")))?;

    let glyphs: Vec<ShapedGlyph> = glyphs_store.borrow_mut().drain(..).collect();
    let captured_faces: Vec<CapturedFace> = faces_store.borrow_mut().drain(..).collect();
    let width_lpx = glyphs.iter().map(|g| g.x_advance_lpx).sum();

    Ok(ShapeResult {
        line: ShapedLine {
            glyphs,
            width_lpx,
            ascent_lpx: metrics.ascent_lpx,
            descent_lpx: metrics.descent_lpx,
            y_offset_lpx: 0.0,
        },
        captured_faces,
    })
}
