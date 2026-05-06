//! Text shaping for CoreText backend.
//!
//! Shapes text using CTLine and extracts glyph positioning information.

use crate::error::TextError;
use crate::types::{FontMetrics, ShapedGlyph, ShapedLine};
use objc2_core_foundation::CFRange;
use objc2_core_text::{
    CTFont, CTLineCreateWithAttributedString, CTLineGetGlyphRuns, CTRunGetAdvances,
    CTRunGetGlyphCount, CTRunGetGlyphs, CTRunGetPositions, kCTFontAttributeName,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSString};

use super::PT_TO_LPX;

/// Shape a line of text into positioned glyphs.
///
/// # Arguments
///
/// * `ct_font` - CoreText font to shape with
/// * `text` - UTF-8 text to shape
/// * `metrics` - Font metrics for line height info
///
/// # Returns
///
/// ShapedLine with glyphs in visual order and total width.
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
        });
    }

    // Create NSString from text
    let ns_string = NSString::from_str(text);

    // Create attributes dictionary with font
    let font_key = unsafe { kCTFontAttributeName };
    let attrs = unsafe { NSDictionary::from_retained_objects(&[font_key], &[ct_font.as_ref()]) };

    // Create attributed string
    let attr_string = unsafe {
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &ns_string,
            Some(&attrs),
        )
    };

    // Create CTLine
    let line = unsafe { CTLineCreateWithAttributedString(&attr_string) };

    // Get glyph runs
    let runs = unsafe { CTLineGetGlyphRuns(&line) };
    let run_count = unsafe { runs.len() };

    let mut glyphs = Vec::new();
    let mut total_width_pt: f64 = 0.0;

    for i in 0..run_count {
        let run = unsafe { runs.get(i) }.expect("run index out of bounds");
        let glyph_count = unsafe { CTRunGetGlyphCount(run) };

        if glyph_count == 0 {
            continue;
        }

        // Allocate buffers for glyph data
        let mut glyph_ids: Vec<u16> = vec![0; glyph_count];
        let mut positions: Vec<objc2_core_graphics::CGPoint> =
            vec![objc2_core_graphics::CGPoint { x: 0.0, y: 0.0 }; glyph_count];
        let mut advances: Vec<objc2_core_graphics::CGSize> = vec![
            objc2_core_graphics::CGSize {
                width: 0.0,
                height: 0.0
            };
            glyph_count
        ];

        // Get glyph IDs
        unsafe {
            CTRunGetGlyphs(
                run,
                CFRange::new(0, glyph_count as isize),
                glyph_ids.as_mut_ptr(),
            );
        }

        // Get positions
        unsafe {
            CTRunGetPositions(
                run,
                CFRange::new(0, glyph_count as isize),
                positions.as_mut_ptr(),
            );
        }

        // Get advances
        unsafe {
            CTRunGetAdvances(
                run,
                CFRange::new(0, glyph_count as isize),
                advances.as_mut_ptr(),
            );
        }

        // Convert to ShapedGlyph
        for j in 0..glyph_count {
            let glyph_id = glyph_ids[j] as u32;
            let x_advance_pt = advances[j].width as f32;
            let x_offset_pt = positions[j].x as f32;
            let y_offset_pt = positions[j].y as f32;

            glyphs.push(ShapedGlyph {
                glyph_id,
                x_advance_lpx: x_advance_pt * PT_TO_LPX,
                x_offset_lpx: x_offset_pt * PT_TO_LPX,
                y_offset_lpx: y_offset_pt * PT_TO_LPX,
            });

            total_width_pt += advances[j].width;
        }
    }

    Ok(ShapedLine {
        glyphs,
        width_lpx: (total_width_pt as f32) * PT_TO_LPX,
        ascent_lpx: metrics.ascent_lpx,
        descent_lpx: metrics.descent_lpx,
    })
}
