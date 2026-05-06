//! Text shaping for DirectWrite backend.
//!
//! Phase 2a uses GetGlyphIndices directly for simple LTR ASCII text.
//! Full IDWriteTextAnalyzer support deferred to Phase 2b.

use crate::{FontMetrics, ShapedGlyph, ShapedLine, TextError};
use windows::Win32::Graphics::DirectWrite::{IDWriteFontFace, DWRITE_GLYPH_METRICS};

/// Shape a line of text using direct glyph lookup.
///
/// Phase 2a: Simple approach using GetGlyphIndices for ASCII/LTR text.
/// Does not handle complex scripts, BiDi, or ligatures.
pub fn shape_line(
    font_face: &IDWriteFontFace,
    em_size_dip: f32,
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

    // Convert UTF-8 to UTF-32 codepoints for GetGlyphIndices
    let codepoints: Vec<u32> = text.chars().map(|c| c as u32).collect();
    let count = codepoints.len() as u32;

    // Get glyph indices for each codepoint
    let mut glyph_indices = vec![0u16; count as usize];
    unsafe {
        font_face.GetGlyphIndices(codepoints.as_ptr(), count, glyph_indices.as_mut_ptr())
    }
    .map_err(|e| TextError::ShapingFailed(format!("GetGlyphIndices: {}", e)))?;

    // Get design glyph metrics for advances
    let mut design_metrics: Vec<DWRITE_GLYPH_METRICS> = vec![Default::default(); count as usize];
    unsafe {
        font_face.GetDesignGlyphMetrics(
            glyph_indices.as_ptr(),
            count,
            design_metrics.as_mut_ptr(),
            false,
        )
    }
    .map_err(|e| TextError::ShapingFailed(format!("GetDesignGlyphMetrics: {}", e)))?;

    // Get font metrics for unit conversion
    let mut font_metrics = Default::default();
    unsafe { font_face.GetMetrics(&mut font_metrics) };

    let units_to_lpx = em_size_dip / font_metrics.designUnitsPerEm as f32;

    // Build ShapedGlyph vec
    let mut glyphs = Vec::with_capacity(count as usize);
    let mut width_lpx = 0.0f32;

    for i in 0..count as usize {
        let advance = design_metrics[i].advanceWidth as f32 * units_to_lpx;
        let glyph = ShapedGlyph {
            glyph_id: glyph_indices[i] as u32,
            x_advance_lpx: advance,
            x_offset_lpx: 0.0,
            y_offset_lpx: 0.0,
        };
        width_lpx += advance;
        glyphs.push(glyph);
    }

    Ok(ShapedLine {
        glyphs,
        width_lpx,
        ascent_lpx: metrics.ascent_lpx,
        descent_lpx: metrics.descent_lpx,
    })
}
