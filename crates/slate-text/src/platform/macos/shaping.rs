//! Text shaping for CoreText backend.
//!
//! Shapes text using CTLine and extracts glyph positioning information.

use crate::FontHandle;
use crate::error::TextError;
use crate::types::{FontId, FontMetrics, ShapedGlyph, ShapedLine};
use objc2_core_foundation::{CFIndex, CFRange, CFRetained, CGPoint, CGSize};
use objc2_core_text::{CTFont, CTRun, kCTFontAttributeName};
use std::ffi::c_void;

/// One CTFont captured from a `CTRun`, paired with the `FontHandle` derived
/// from its PostScript-name hash + the line's size/scale. The font is
/// `CFRetained` so it outlives the borrowed reference returned by
/// `CFDictionaryGetValue`.
pub(crate) struct CapturedFont {
    pub(crate) handle: FontHandle,
    pub(crate) font: CFRetained<CTFont>,
}

/// Result of shaping a CoreText line.
pub(crate) struct ShapeResult {
    pub(crate) line: ShapedLine,
    pub(crate) captured_fonts: Vec<CapturedFont>,
}

// External CoreFoundation functions
unsafe extern "C" {
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        c_str: *const i8,
        encoding: u32,
    ) -> *const c_void;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const *const c_void,
        values: *const *const c_void,
        num_values: CFIndex,
        key_callbacks: *const c_void,
        value_callbacks: *const c_void,
    ) -> *const c_void;
    fn CFAttributedStringCreate(
        alloc: *const c_void,
        string: *const c_void,
        attributes: *const c_void,
    ) -> *const c_void;
    fn CTLineCreateWithAttributedString(attr_string: *const c_void) -> *const c_void;
    fn CTLineGetGlyphRuns(line: *const c_void) -> *const c_void;
    fn CFArrayGetCount(array: *const c_void) -> CFIndex;
    fn CFArrayGetValueAtIndex(array: *const c_void, idx: CFIndex) -> *const c_void;
    fn CTRunGetGlyphCount(run: *const c_void) -> CFIndex;
    fn CTRunGetGlyphs(run: *const c_void, range: CFRange, buffer: *mut u16);
    fn CTRunGetPositions(run: *const c_void, range: CFRange, buffer: *mut CGPoint);
    fn CTRunGetAdvances(run: *const c_void, range: CFRange, buffer: *mut CGSize);
    /// Returns a pointer to an array of UTF-16 source indices, one per glyph
    /// in the run. Indices reference the originating attributed string (which
    /// for our shape_line is the UTF-16 encoding of the input `&str`).
    ///
    /// Returns NULL when the run does not store indices in a directly
    /// addressable form — typical for substitute / fallback runs. Callers must
    /// fall back to `CTRunGetStringIndices` (the copy variant) in that case.
    fn CTRunGetStringIndicesPtr(run: *const c_void) -> *const CFIndex;

    /// Copy variant of `CTRunGetStringIndicesPtr`. Always succeeds when the run
    /// has indices (CoreText always tracks them — only the direct-pointer
    /// optimisation may be unavailable). `range` selects a contiguous slice of
    /// glyph indices; `buffer` must have capacity for `range.length` entries.
    fn CTRunGetStringIndices(run: *const c_void, range: CFRange, buffer: *mut CFIndex);
    fn CTRunGetAttributes(run: *const c_void) -> *const c_void;
    fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFRetain(cf: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);

    /// Canonical CF retain-release callbacks for dictionary keys.
    /// Using these (vs null) ensures CF applies proper memory management
    /// for CF-type keys (retain on insert, release on remove).
    static kCFTypeDictionaryKeyCallBacks: c_void;

    /// Canonical CF retain-release callbacks for dictionary values.
    static kCFTypeDictionaryValueCallBacks: c_void;
}

const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;

/// RAII guard for an owned CoreFoundation object.
///
/// On drop, calls `CFRelease` on the wrapped pointer unless it is null.
/// Does **not** wrap borrowed refs returned by functions like
/// `CTLineGetGlyphRuns` / `CFArrayGetValueAtIndex` — those must not be released.
struct ScopedCf(*const c_void);

impl ScopedCf {
    /// Return the inner pointer (may be null).
    fn ptr(&self) -> *const c_void {
        self.0
    }

    /// Return true when the wrapped pointer is null (creation failed).
    fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

impl Drop for ScopedCf {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // Safety: pointer was obtained from a CF Create/Copy function and
            // ownership was transferred to this guard at construction time.
            unsafe { CFRelease(self.0) };
        }
    }
}

/// Shape a line of text into positioned glyphs.
///
/// `size_lpx` / `scale` parametrize the `FontHandle`s recorded on each glyph
/// and each `CapturedFont` so the cache key convention matches the primary.
pub(crate) fn shape_line(
    ct_font: &CTFont,
    text: &str,
    metrics: &FontMetrics,
    size_lpx: f32,
    scale: f32,
) -> Result<ShapeResult, TextError> {
    // Empty string guard
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
            captured_fonts: Vec::new(),
        });
    }

    // Create C string from text
    let c_str = std::ffi::CString::new(text)
        .map_err(|_| TextError::ShapingFailed("Text contains null bytes".into()))?;

    // UTF-16 → UTF-8 byte map. `CTRunGetStringIndicesPtr` reports indices into
    // the CFString (UTF-16); we translate to HarfBuzz-style UTF-8 cluster
    // values for `ShapedGlyph::cluster`.
    let utf16_to_utf8 = crate::cluster::utf16_to_utf8_byte_map(text);

    // Safety: all CF/CT calls follow Create/Copy ownership rules; owned objects
    // are wrapped in ScopedCf for deterministic release.
    unsafe {
        // Create CFString from text
        let cf_string = ScopedCf(CFStringCreateWithCString(
            std::ptr::null(),
            c_str.as_ptr(),
            K_CF_STRING_ENCODING_UTF8,
        ));
        if cf_string.is_null() {
            return Err(TextError::ShapingFailed("Failed to create CFString".into()));
        }

        // Create attributes dictionary with font.
        // Use kCFTypeDictionaryKeyCallBacks / kCFTypeDictionaryValueCallBacks so
        // CF correctly retains/releases the CF-type key and value on its own.
        let font_key = kCTFontAttributeName as *const _ as *const c_void;
        let font_value = ct_font as *const CTFont as *const c_void;
        let keys = [font_key];
        let values = [font_value];

        let attrs = ScopedCf(CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks as *const c_void,
            &kCFTypeDictionaryValueCallBacks as *const c_void,
        ));
        // attrs null-check: CFDictionaryCreate only fails under extreme OOM;
        // proceed anyway — CFAttributedStringCreate handles a null attrs gracefully
        // by producing an empty attributes dictionary, which is safe here.

        // Create attributed string; cf_string and attrs are still alive here.
        let attr_string = ScopedCf(CFAttributedStringCreate(
            std::ptr::null(),
            cf_string.ptr(),
            attrs.ptr(),
        ));
        // cf_string and attrs are released when they go out of scope (ScopedCf Drop).
        // Drop them explicitly now so lifetime is obvious.
        drop(attrs);
        drop(cf_string);

        if attr_string.is_null() {
            return Err(TextError::ShapingFailed(
                "Failed to create CFAttributedString".into(),
            ));
        }

        // Create CTLine
        let line = ScopedCf(CTLineCreateWithAttributedString(attr_string.ptr()));
        drop(attr_string);

        if line.is_null() {
            return Err(TextError::ShapingFailed("Failed to create CTLine".into()));
        }

        // Get glyph runs.
        // CTLineGetGlyphRuns returns a *borrowed* reference owned by `line`;
        // do NOT wrap it in ScopedCf or release it.
        let runs = CTLineGetGlyphRuns(line.ptr());
        let run_count = CFArrayGetCount(runs);

        let mut glyphs = Vec::new();
        let mut captured_fonts: Vec<CapturedFont> = Vec::new();
        let mut total_width_pt: f64 = 0.0;

        let primary_handle = super::font_id::font_handle_from_ct_font(ct_font, size_lpx, scale);

        for i in 0..run_count {
            // CFArrayGetValueAtIndex also returns a borrowed reference.
            let run = CFArrayGetValueAtIndex(runs, i);
            if run.is_null() {
                continue;
            }
            let _run_ref: &CTRun = &*(run as *const CTRun);
            let glyph_count = CTRunGetGlyphCount(run) as usize;

            if glyph_count == 0 {
                continue;
            }

            // Per-run font extraction. CTRunGetAttributes returns a borrowed
            // CFDictionary owned by the run; the kCTFontAttributeName entry
            // inside it is also borrowed. We CFRetain to extend lifetime.
            let attrs = CTRunGetAttributes(run);
            let font_key = kCTFontAttributeName as *const _ as *const c_void;
            let run_font_handle = if !attrs.is_null() {
                let run_font_ptr = CFDictionaryGetValue(attrs, font_key);
                if run_font_ptr.is_null() {
                    FontHandle::default()
                } else {
                    let run_font_ref: &CTFont = &*(run_font_ptr as *const CTFont);
                    let handle =
                        super::font_id::font_handle_from_ct_font(run_font_ref, size_lpx, scale);
                    // Register the substitute font once per unique handle.
                    // Skip the primary — caller already holds it.
                    if handle != primary_handle
                        && !captured_fonts.iter().any(|cf| cf.handle == handle)
                    {
                        // CFRetain the borrowed CTFont so the CFRetained guard
                        // owns a strong reference independent of the run.
                        let _ = CFRetain(run_font_ptr);
                        let nonnull = std::ptr::NonNull::new(run_font_ptr as *mut CTFont)
                            .expect("non-null after null check above");
                        let retained = CFRetained::from_raw(nonnull);
                        captured_fonts.push(CapturedFont {
                            handle,
                            font: retained,
                        });
                    }
                    handle
                }
            } else {
                FontHandle::default()
            };

            // Allocate buffers for glyph data
            let mut glyph_ids: Vec<u16> = vec![0; glyph_count];
            let mut positions: Vec<CGPoint> = vec![CGPoint { x: 0.0, y: 0.0 }; glyph_count];
            let mut advances: Vec<CGSize> = vec![
                CGSize {
                    width: 0.0,
                    height: 0.0
                };
                glyph_count
            ];

            let range = CFRange::new(0, glyph_count as CFIndex);

            // Get glyph IDs
            CTRunGetGlyphs(run, range, glyph_ids.as_mut_ptr());

            // Get positions
            CTRunGetPositions(run, range, positions.as_mut_ptr());

            // Get advances
            CTRunGetAdvances(run, range, advances.as_mut_ptr());

            // Per-glyph UTF-16 source indices. Prefer the direct pointer (no
            // copy); fall back to the copy variant when CoreText can't expose
            // a contiguous internal buffer — happens for substitute / fallback
            // runs (emoji, regional-indicator pairs, ZWJ sequences, NFD with
            // combining marks). Falling back is critical: without it those
            // glyphs all collapse to cluster=0 and `pixel_x_at_byte` walks the
            // entire run before finding a glyph past the requested byte.
            let indices_ptr = CTRunGetStringIndicesPtr(run);
            let mut indices_owned: Vec<CFIndex> = Vec::new();
            let string_indices: &[CFIndex] = if !indices_ptr.is_null() {
                // SAFETY: pointer is valid for `glyph_count` entries for the
                // lifetime of `run`, which outlives this loop iteration.
                std::slice::from_raw_parts(indices_ptr, glyph_count)
            } else {
                indices_owned.resize(glyph_count, 0);
                CTRunGetStringIndices(run, range, indices_owned.as_mut_ptr());
                &indices_owned[..]
            };

            // Convert to ShapedGlyph.
            //
            // CoreText `positions[j]` is the absolute pen position of glyph j
            // relative to the CTLine origin (cumulative across the line, with
            // per-glyph nudges already baked in). Store it directly as
            // `position_lpx` — this is the canonical absolute form.
            for j in 0..glyph_count {
                let glyph_id = glyph_ids[j] as u32;
                let x_advance_pt = advances[j].width as f32;
                let position = [positions[j].x as f32, positions[j].y as f32];

                let utf16_pos = string_indices[j] as usize;
                let cluster = utf16_to_utf8.get(utf16_pos).copied().unwrap_or(0);

                glyphs.push(ShapedGlyph {
                    glyph_id,
                    font_id: FontId::PRIMARY,
                    font_handle: run_font_handle,
                    x_advance_lpx: x_advance_pt,
                    position_lpx: position,
                    cluster,
                    // Direction is assigned per level-run by the segmenter;
                    // `shape_line` shapes one already-resolved span, so the
                    // raw shaper output stays at the LTR default here.
                    direction: crate::types::Direction::Ltr,
                });

                total_width_pt += advances[j].width;
            }
        }

        // `line` goes out of scope here and calls CFRelease via ScopedCf::drop.

        Ok(ShapeResult {
            line: ShapedLine {
                glyphs,
                width_lpx: total_width_pt as f32,
                ascent_lpx: metrics.ascent_lpx,
                descent_lpx: metrics.descent_lpx,
                y_offset_lpx: 0.0,
                base_direction: crate::types::Direction::Ltr,
                runs: Vec::new(),
            },
            captured_fonts,
        })
    }
}
