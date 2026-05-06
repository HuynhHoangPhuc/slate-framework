//! Glyph rasterization via IDWriteGlyphRunAnalysis.
//!
//! Uses ALIASED_1x1 texture type for greyscale AA (not ClearType RGB).
//! Sub-pixel variants via DWRITE_MATRIX.dx translation.

use crate::{GlyphBitmap, TextError};
use std::mem::ManuallyDrop;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_GLYPH_METRICS, DWRITE_GLYPH_OFFSET, DWRITE_GLYPH_RUN, DWRITE_GRID_FIT_MODE_DEFAULT,
    DWRITE_MATRIX, DWRITE_MEASURING_MODE_NATURAL, DWRITE_RENDERING_MODE1_NATURAL,
    DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE, DWRITE_TEXTURE_ALIASED_1x1, IDWriteFactory5,
    IDWriteFontFace,
};

/// Rasterize a single glyph with sub-pixel variant offset.
///
/// `variant` must be 0..=3, representing 0.0/0.25/0.5/0.75 pixel X offset.
pub fn rasterize(
    factory: &IDWriteFactory5,
    font_face: &IDWriteFontFace,
    em_size_dip: f32,
    pixels_per_dip: f32,
    glyph_id: u16,
    variant: u8,
) -> Result<GlyphBitmap, TextError> {
    // Variant range guard
    if variant > 3 {
        return Err(TextError::RasterizationFailed(
            "variant out of range (must be 0-3)".into(),
        ));
    }

    // Get advance for this glyph
    let advance_x_lpx = get_glyph_advance(font_face, glyph_id, em_size_dip)?;

    // Use helper to safely manage DWRITE_GLYPH_RUN lifetime
    with_glyph_run(font_face, em_size_dip, glyph_id, |run| {
        // Sub-pixel offset via matrix translation
        let transform = DWRITE_MATRIX {
            m11: 1.0,
            m12: 0.0,
            m21: 0.0,
            m22: 1.0,
            dx: variant as f32 * 0.25,
            dy: 0.0,
        };

        // Create glyph run analysis (IDWriteFactory5 version with more parameters)
        let analysis = unsafe {
            factory.CreateGlyphRunAnalysis(
                run,
                Some(&transform),
                DWRITE_RENDERING_MODE1_NATURAL,
                DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_GRID_FIT_MODE_DEFAULT,
                DWRITE_TEXT_ANTIALIAS_MODE_GRAYSCALE,
                0.0, // baseline origin X
                0.0, // baseline origin Y
            )
        }
        .map_err(|e| TextError::RasterizationFailed(format!("CreateGlyphRunAnalysis: {}", e)))?;

        // Get texture bounds
        let bounds = unsafe { analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1) }
            .map_err(|e| TextError::RasterizationFailed(format!("GetAlphaTextureBounds: {}", e)))?;

        // Empty-rect guard (whitespace glyphs)
        if bounds.right <= bounds.left || bounds.bottom <= bounds.top {
            return Ok(GlyphBitmap {
                width: 0,
                height: 0,
                bearing_x_lpx: 0.0,
                bearing_y_lpx: 0.0,
                advance_x_lpx,
                alpha: vec![],
            });
        }

        let width = (bounds.right - bounds.left) as u32;
        let height = (bounds.bottom - bounds.top) as u32;
        let mut alpha = vec![0u8; (width * height) as usize];

        // Create alpha texture
        unsafe { analysis.CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &bounds, &mut alpha) }
            .map_err(|e| TextError::RasterizationFailed(format!("CreateAlphaTexture: {}", e)))?;

        // Convert bearing from physical pixels to logical pixels
        let bearing_x_lpx = bounds.left as f32 / pixels_per_dip;
        let bearing_y_lpx = -(bounds.top as f32) / pixels_per_dip;

        Ok(GlyphBitmap {
            width,
            height,
            bearing_x_lpx,
            bearing_y_lpx,
            advance_x_lpx,
            alpha,
        })
    })
}

/// Get glyph advance width in logical pixels.
fn get_glyph_advance(
    font_face: &IDWriteFontFace,
    glyph_id: u16,
    em_size_dip: f32,
) -> Result<f32, TextError> {
    let glyph_indices = [glyph_id];
    let mut metrics: [DWRITE_GLYPH_METRICS; 1] = [Default::default()];

    unsafe {
        font_face.GetDesignGlyphMetrics(glyph_indices.as_ptr(), 1, metrics.as_mut_ptr(), false)
    }
    .map_err(|e| TextError::RasterizationFailed(format!("GetDesignGlyphMetrics: {}", e)))?;

    let mut font_metrics = Default::default();
    unsafe { font_face.GetMetrics(&mut font_metrics) };

    let units_to_lpx = em_size_dip / font_metrics.designUnitsPerEm as f32;
    Ok(metrics[0].advanceWidth as f32 * units_to_lpx)
}

/// Helper to safely manage DWRITE_GLYPH_RUN with ManuallyDrop fontFace.
fn with_glyph_run<R>(
    face: &IDWriteFontFace,
    em_size_dip: f32,
    glyph_id: u16,
    body: impl FnOnce(&DWRITE_GLYPH_RUN) -> R,
) -> R {
    let glyph_indices = [glyph_id];
    let glyph_advances = [0.0f32];
    let glyph_offsets = [DWRITE_GLYPH_OFFSET::default()];

    let mut run = DWRITE_GLYPH_RUN {
        fontFace: ManuallyDrop::new(Some(face.clone())),
        fontEmSize: em_size_dip,
        glyphCount: 1,
        glyphIndices: glyph_indices.as_ptr(),
        glyphAdvances: glyph_advances.as_ptr(),
        glyphOffsets: glyph_offsets.as_ptr(),
        isSideways: false.into(),
        bidiLevel: 0,
    };

    let result = body(&run);

    // Release the cloned face exactly once
    unsafe {
        ManuallyDrop::drop(&mut run.fontFace);
    }

    result
}
