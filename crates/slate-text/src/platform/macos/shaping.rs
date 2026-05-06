//! Text shaping for CoreText backend.
//!
//! Shapes text using CTLine and extracts glyph positioning information.

use crate::error::TextError;
use crate::types::{FontId, FontMetrics, ShapedGlyph, ShapedLine};
use objc2_core_foundation::{CFIndex, CFRange, CGPoint, CGSize};
use objc2_core_text::{CTFont, CTRun, kCTFontAttributeName};
use std::ffi::c_void;

use super::PT_TO_LPX;

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
    fn CFRelease(cf: *const c_void);
}

const K_CF_STRING_ENCODING_UTF8: u32 = 0x08000100;

/// Shape a line of text into positioned glyphs.
pub fn shape_line(
    ct_font: &CTFont,
    text: &str,
    metrics: &FontMetrics,
) -> Result<ShapedLine, TextError> {
    // Empty string guard
    if text.is_empty() {
        return Ok(ShapedLine {
            glyphs: vec![],
            width_lpx: 0.0,
            ascent_lpx: metrics.ascent_lpx,
            descent_lpx: metrics.descent_lpx,
            y_offset_lpx: 0.0,
        });
    }

    // Create C string from text
    let c_str = std::ffi::CString::new(text).map_err(|_| {
        TextError::ShapingFailed("Text contains null bytes".into())
    })?;

    unsafe {
        // Create CFString from text
        let cf_string = CFStringCreateWithCString(
            std::ptr::null(),
            c_str.as_ptr(),
            K_CF_STRING_ENCODING_UTF8,
        );
        if cf_string.is_null() {
            return Err(TextError::ShapingFailed("Failed to create CFString".into()));
        }

        // Create attributes dictionary with font
        let font_key = kCTFontAttributeName as *const _ as *const c_void;
        let font_value = ct_font as *const CTFont as *const c_void;
        let keys = [font_key];
        let values = [font_value];

        let attrs = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            std::ptr::null(), // kCFTypeDictionaryKeyCallBacks
            std::ptr::null(), // kCFTypeDictionaryValueCallBacks
        );

        // Create attributed string
        let attr_string = CFAttributedStringCreate(std::ptr::null(), cf_string, attrs);
        CFRelease(cf_string);
        if !attrs.is_null() {
            CFRelease(attrs);
        }
        if attr_string.is_null() {
            return Err(TextError::ShapingFailed("Failed to create CFAttributedString".into()));
        }

        // Create CTLine
        let line = CTLineCreateWithAttributedString(attr_string);
        CFRelease(attr_string);
        if line.is_null() {
            return Err(TextError::ShapingFailed("Failed to create CTLine".into()));
        }

        // Get glyph runs
        let runs = CTLineGetGlyphRuns(line);
        let run_count = CFArrayGetCount(runs);

        let mut glyphs = Vec::new();
        let mut total_width_pt: f64 = 0.0;

        for i in 0..run_count {
            let run = CFArrayGetValueAtIndex(runs, i);
            if run.is_null() {
                continue;
            }
            let _run_ref: &CTRun = &*(run as *const CTRun);
            let glyph_count = CTRunGetGlyphCount(run) as usize;

            if glyph_count == 0 {
                continue;
            }

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

            // Convert to ShapedGlyph
            for j in 0..glyph_count {
                let glyph_id = glyph_ids[j] as u32;
                let x_advance_pt = advances[j].width as f32;
                let x_offset_pt = positions[j].x as f32;
                let y_offset_pt = positions[j].y as f32;

                glyphs.push(ShapedGlyph {
                    glyph_id,
                    font_id: FontId::PRIMARY,
                    x_advance_lpx: x_advance_pt * PT_TO_LPX,
                    x_offset_lpx: x_offset_pt * PT_TO_LPX,
                    y_offset_lpx: y_offset_pt * PT_TO_LPX,
                });

                total_width_pt += advances[j].width;
            }
        }

        CFRelease(line);

        Ok(ShapedLine {
            glyphs,
            width_lpx: (total_width_pt as f32) * PT_TO_LPX,
            ascent_lpx: metrics.ascent_lpx,
            descent_lpx: metrics.descent_lpx,
            y_offset_lpx: 0.0,
        })
    }
}
