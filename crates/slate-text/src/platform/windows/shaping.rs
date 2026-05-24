//! Text shaping via IDWriteTextLayout with custom IDWriteTextRenderer.
//!
//! Uses the full DirectWrite shaping pipeline for kerned, GPOS-adjusted glyph advances.

use crate::types::FontId;
use crate::{FontHandle, FontMetrics, ShapedGlyph, ShapedLine, TextError};
use std::cell::RefCell;
use std::rc::Rc;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_GLYPH_RUN, DWRITE_GLYPH_RUN_DESCRIPTION, DWRITE_MATRIX, DWRITE_MEASURING_MODE,
    DWRITE_READING_DIRECTION_LEFT_TO_RIGHT, DWRITE_READING_DIRECTION_RIGHT_TO_LEFT,
    DWRITE_STRIKETHROUGH, DWRITE_UNDERLINE, IDWriteFactory5, IDWriteFontFace, IDWriteInlineObject,
    IDWritePixelSnapping_Impl, IDWriteTextFormat, IDWriteTextRenderer, IDWriteTextRenderer_Impl,
};
use windows::core::{BOOL, IUnknown, Ref, Result, implement};

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
    /// UTF-16 code-unit → UTF-8 byte-offset map for the layout's source text.
    /// Used to translate `DWRITE_GLYPH_RUN_DESCRIPTION::clusterMap` (UTF-16
    /// indexed) into HarfBuzz-style UTF-8 cluster values on each glyph.
    utf16_to_utf8: Vec<u32>,
}

impl ShapingRenderer {
    /// Create a new renderer with shared glyph + face storage.
    pub(crate) fn new(
        glyphs: GlyphStore,
        faces: FaceStore,
        size_lpx: f32,
        scale: f32,
        primary_handle: FontHandle,
        utf16_to_utf8: Vec<u32>,
    ) -> Self {
        Self {
            glyphs,
            faces,
            size_lpx,
            scale,
            primary_handle,
            utf16_to_utf8,
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
        baselineoriginx: f32,
        _baselineoriginy: f32,
        _measuringmode: DWRITE_MEASURING_MODE,
        glyphrun: *const DWRITE_GLYPH_RUN,
        glyphrundescription: *const DWRITE_GLYPH_RUN_DESCRIPTION,
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

        // Derive per-glyph cluster (UTF-8 byte offset into the source string)
        // from `DWRITE_GLYPH_RUN_DESCRIPTION::clusterMap`. The cluster map is
        // indexed by UTF-16 char position within this run; each entry is the
        // glyph index (within the run) the char maps to. Inverse: for each
        // glyph, take the smallest char index pointing at it; glyphs that no
        // char points at directly belong to the preceding glyph's cluster
        // (HarfBuzz forward-fill convention for multi-glyph clusters).
        // `glyphrundescription` is null for non-text runs (inline objects);
        // those have count == 0 and were already short-circuited above.
        let clusters: Vec<u32> = if !glyphrundescription.is_null() {
            let desc = unsafe { &*glyphrundescription };
            let text_position = desc.textPosition as usize;
            let string_length = desc.stringLength as usize;
            let cluster_map = if !desc.clusterMap.is_null() && string_length > 0 {
                Some(unsafe { std::slice::from_raw_parts(desc.clusterMap, string_length) })
            } else {
                None
            };

            let mut glyph_to_char: Vec<Option<u32>> = vec![None; count];
            if let Some(cm) = cluster_map {
                for (j, &g) in cm.iter().enumerate() {
                    let g = g as usize;
                    if g < count && glyph_to_char[g].is_none() {
                        glyph_to_char[g] = Some(j as u32);
                    }
                }
                // Forward-fill: glyphs at decomposition tails inherit the
                // char position of the cluster's leading glyph. The `last = 0`
                // seed relies on DirectWrite's invariant that clusterMap[0] is
                // always 0 (run starts at its first char); if that ever fails,
                // glyph 0 maps to text_position+0 which is still the
                // leading-char cluster — so do not "fix" this with an Option
                // wrapper that would break the fallback.
                let mut last: u32 = 0;
                for slot in glyph_to_char.iter_mut() {
                    match slot {
                        Some(v) => last = *v,
                        None => *slot = Some(last),
                    }
                }
            }

            glyph_to_char
                .iter()
                .map(|slot| {
                    let char_in_run = slot.unwrap_or(0) as usize;
                    let utf16_pos = text_position + char_in_run;
                    self.utf16_to_utf8.get(utf16_pos).copied().unwrap_or(0)
                })
                .collect()
        } else {
            vec![0u32; count]
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
                let id = super::font_id::idwrite_font_face_id(face);
                FontHandle::from_face_id(id, self.size_lpx, self.scale)
            }
            None => FontHandle::default(),
        };

        // Record the face for backend registry insertion (post-Draw). Skip
        // entries matching the primary — the primary path in `run_builder`
        // routes through `self.font` directly and never queries the registry,
        // so storing it just wastes an `extract_metrics` + heap alloc.
        // Dedup against already-captured substitutes is per-callback; cross-call
        // dedup happens at the HashMap level in `mod.rs` via `entry().or_insert_with`.
        // Pointer-stability: `IDWriteFactory5` canonicalizes substitute font faces
        // per process — repeated substitute callbacks for the same face family
        // return the same COM pointer. The PSName-hash keying in
        // `font_id::idwrite_font_face_id` is therefore safe, and its
        // `as_raw() as u64` fallback is also empirically stable.
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

        // DirectWrite reports `advances[i]` (per-glyph advance) and optional
        // `offsets[i]` (HarfBuzz-style relative nudge for combining marks).
        // Accumulate into the canonical absolute `position_lpx` form so the
        // renderer's contract is identical to CoreText: paint at
        // `baseline + position_lpx` with no pen accumulator on the consumer
        // side. `ascenderOffset` is positive-up; we negate so `position_lpx[1]`
        // matches CoreText's baseline-relative Y convention (Y-up at the
        // shaper layer — the renderer applies its own axis flip).
        //
        // Pen origin per callback: DirectWrite invokes this once per shaped run
        // and passes `baselineoriginx` already positioned at that run's visual
        // left edge (relative to the `Draw` origin, which we pass as 0.0). Seed
        // the pen from it rather than summing prior advances — a sum assumes
        // every run is laid out left-to-right in callback order, which is false
        // for RTL/bidi: DirectWrite emits an RTL run's rightmost glyphs first
        // and steps the origin leftward, so the sum would mis-stack the runs.
        // Using the reported origin places each run where the layout put it.
        let mut pen: f32 = baselineoriginx;
        for i in 0..count {
            let (off_x, off_y) = offsets
                .map(|o| (o[i].advanceOffset, -o[i].ascenderOffset))
                .unwrap_or((0.0, 0.0));
            let position = [pen + off_x, off_y];
            glyphs.push(ShapedGlyph {
                glyph_id: indices[i] as u32,
                font_id: FontId::PRIMARY,
                font_handle,
                x_advance_lpx: advances[i],
                position_lpx: position,
                cluster: clusters[i],
                // Per-run direction is assigned by the segmenter from the
                // resolved bidi level; this raw shaper output carries no
                // direction of its own, so it stays at the LTR default.
                direction: crate::types::Direction::Ltr,
            });
            pen += advances[i];
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
///
/// `forced_direction = Some(dir)` pins the layout's reading direction so an
/// isolated, already-resolved level-run cannot be re-detected with a different
/// base by DirectWrite's own bidi pass; `None` lets it auto-detect (used by
/// direction-agnostic callers).
// Internal shaping primitive: factory + format + the font's cache-key params
// (size/scale/handle) are all genuinely needed per call; bundling them into a
// struct would only move the argument list, not shrink the real coupling.
#[allow(clippy::too_many_arguments)]
pub(crate) fn shape_line(
    factory: &IDWriteFactory5,
    text_format: &IDWriteTextFormat,
    text: &str,
    metrics: &FontMetrics,
    size_lpx: f32,
    scale: f32,
    primary_handle: FontHandle,
    forced_direction: Option<crate::types::Direction>,
) -> std::result::Result<ShapeResult, TextError> {
    if text.is_empty() {
        return Ok(ShapeResult {
            line: ShapedLine {
                glyphs: vec![],
                width_lpx: 0.0,
                ascent_lpx: metrics.ascent_lpx,
                descent_lpx: metrics.descent_lpx,
                y_offset_lpx: 0.0,
                base_direction: crate::types::Direction::Ltr,
                runs: Vec::new(),
            },
            captured_faces: Vec::new(),
        });
    }

    // UTF-8 → UTF-16
    let wide: Vec<u16> = text.encode_utf16().collect();

    // UTF-16 → UTF-8 byte map for cluster derivation in DrawGlyphRun.
    let utf16_to_utf8 = crate::cluster::utf16_to_utf8_byte_map(text);

    // CreateTextLayout takes &[u16]
    let layout = unsafe { factory.CreateTextLayout(&wide, text_format, f32::MAX, f32::MAX) }
        .map_err(|e| TextError::ShapingFailed(format!("CreateTextLayout: {e}")))?;

    // Pin the reading direction for an already-resolved level-run so DirectWrite
    // does not re-detect a different base for the isolated substring. The layout
    // extends IDWriteTextFormat, which declares SetReadingDirection.
    if let Some(direction) = forced_direction {
        let reading_direction = match direction {
            crate::types::Direction::Ltr => DWRITE_READING_DIRECTION_LEFT_TO_RIGHT,
            crate::types::Direction::Rtl => DWRITE_READING_DIRECTION_RIGHT_TO_LEFT,
        };
        unsafe { layout.SetReadingDirection(reading_direction) }
            .map_err(|e| TextError::ShapingFailed(format!("SetReadingDirection: {e}")))?;
    }

    // An RTL run anchors its glyph origins at the layout box's RIGHT edge. With
    // the f32::MAX max-width above, that edge sits at ~f32::MAX, where the float
    // grid is so coarse that adding a glyph advance is a no-op (every glyph
    // collapses onto the same origin) and the origins themselves overflow to
    // infinity downstream — RTL text then paints far off-screen (blank). LTR is
    // unaffected because it anchors at the left edge (0) regardless of width.
    //
    // Measure the line's natural width and pin the box to it, so the right edge
    // is a small finite coordinate where per-glyph advances are preserved and
    // origins stay word-origin-relative — the contract `assemble_visual_line`
    // relies on (it shifts each segment's glyphs by the running pen). A 1 lpx
    // margin absorbs sub-DIP rounding so the trailing glyph never wraps.
    let mut text_metrics = windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS::default();
    unsafe { layout.GetMetrics(&mut text_metrics) }
        .map_err(|e| TextError::ShapingFailed(format!("GetMetrics: {e}")))?;
    // `width` excludes trailing whitespace; `widthIncludingTrailingWhitespace`
    // covers a space-only segment. RTL reports the real content in `width` while
    // `widthIncludingTrailingWhitespace` comes back 0, so take the max to cover
    // both: pure-space LTR runs and RTL content runs alike.
    let measured = text_metrics
        .width
        .max(text_metrics.widthIncludingTrailingWhitespace);
    // A pure-whitespace RTL segment (e.g. the space between two Arabic words —
    // UAX #9 gives it an RTL level) reports 0 in BOTH metric fields, which would
    // leave the f32::MAX box in place and collapse a multi-space run onto one
    // origin. Fall back to a finite bound that comfortably exceeds any
    // whitespace run (each space ≪ one em) so the box stays small enough to
    // preserve per-glyph advances yet never wraps the content. Always set a
    // finite max width so the RTL right-edge anchor can never reach f32::MAX.
    let box_width = if measured.is_finite() && measured > 0.0 {
        measured
    } else {
        (wide.len() as f32 + 1.0) * size_lpx
    };
    unsafe { layout.SetMaxWidth(box_width + 1.0) }
        .map_err(|e| TextError::ShapingFailed(format!("SetMaxWidth: {e}")))?;

    // Rc<RefCell> shared state for glyph + face accumulation
    let glyphs_store: GlyphStore = Rc::new(RefCell::new(Vec::new()));
    let faces_store: FaceStore = Rc::new(RefCell::new(Vec::new()));
    let renderer = ShapingRenderer::new(
        Rc::clone(&glyphs_store),
        Rc::clone(&faces_store),
        size_lpx,
        scale,
        primary_handle,
        utf16_to_utf8,
    );
    let renderer_iface: IDWriteTextRenderer = renderer.into();

    // Draw() invokes DrawGlyphRun callback(s)
    unsafe { layout.Draw(None, &renderer_iface, 0.0, 0.0) }
        .map_err(|e| TextError::ShapingFailed(format!("IDWriteTextLayout::Draw: {e}")))?;

    let mut glyphs: Vec<ShapedGlyph> = glyphs_store.borrow_mut().drain(..).collect();
    let captured_faces: Vec<CapturedFace> = faces_store.borrow_mut().drain(..).collect();

    // Anchor the segment to a word-origin-relative pen (min x = 0). LTR already
    // arrives at 0; an RTL run arrives offset by the (now finite) box width
    // because it anchors at the right edge. `assemble_visual_line` shifts each
    // segment by the running line pen and so requires 0-based glyph origins —
    // subtract the minimum x to satisfy that contract for every direction.
    if let Some(min_x) = glyphs
        .iter()
        .map(|g| g.position_lpx[0])
        .fold(None, |acc: Option<f32>, x| {
            Some(acc.map_or(x, |a| a.min(x)))
        })
        && min_x != 0.0
    {
        for g in &mut glyphs {
            g.position_lpx[0] -= min_x;
        }
    }

    let width_lpx = glyphs.iter().map(|g| g.x_advance_lpx).sum();

    Ok(ShapeResult {
        line: ShapedLine {
            glyphs,
            width_lpx,
            ascent_lpx: metrics.ascent_lpx,
            descent_lpx: metrics.descent_lpx,
            y_offset_lpx: 0.0,
            base_direction: crate::types::Direction::Ltr,
            runs: Vec::new(),
        },
        captured_faces,
    })
}
