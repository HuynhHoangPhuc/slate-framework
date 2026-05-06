//! Font loading for DirectWrite backend.
//!
//! Uses IDWriteInMemoryFontFileLoader (DirectWrite 5+) to load fonts from static byte slices.
//! System font lookup deferred to Phase 2b.

use crate::{FontHandle, FontMetrics, TextError};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_FACE_TYPE, DWRITE_FONT_FILE_TYPE, DWRITE_FONT_SIMULATIONS_NONE, IDWriteFactory5,
    IDWriteFontFace, IDWriteFontFile, IDWriteInMemoryFontFileLoader,
};
use windows::core::Interface;

use super::DirectWriteFont;

/// Load a font from raw TTF/OTF bytes using IDWriteInMemoryFontFileLoader.
pub fn load_font_from_bytes(
    factory: &IDWriteFactory5,
    loader: &IDWriteInMemoryFontFileLoader,
    bytes: &'static [u8],
    size_lpx: f32,
    scale: f32,
) -> Result<DirectWriteFont, TextError> {
    // Create font file from memory using the in-memory loader
    let font_file: IDWriteFontFile = unsafe {
        loader.CreateInMemoryFontFileReference(
            factory,
            bytes.as_ptr() as *const _,
            bytes.len() as u32,
            None, // no owner object
        )
    }
    .map_err(|e| TextError::FontFileLoad(format!("CreateInMemoryFontFileReference: {}", e)))?;

    // Analyze the font file to get face type and count
    let mut is_supported = false.into();
    let mut file_type = DWRITE_FONT_FILE_TYPE::default();
    let mut face_type = DWRITE_FONT_FACE_TYPE::default();
    let mut face_count = 0u32;

    unsafe {
        font_file.Analyze(
            &mut is_supported,
            &mut file_type,
            Some(&mut face_type),
            &mut face_count,
        )
    }
    .map_err(|e| TextError::FontFileLoad(format!("Analyze: {}", e)))?;

    if !is_supported.as_bool() || face_count == 0 {
        return Err(TextError::FontFileLoad("Font file not supported".into()));
    }

    // Create font face from the file (index 0)
    let font_files = [Some(font_file)];
    let font_face: IDWriteFontFace =
        unsafe { factory.CreateFontFace(face_type, &font_files, 0, DWRITE_FONT_SIMULATIONS_NONE) }
            .map_err(|e| TextError::FontFileLoad(format!("CreateFontFace: {}", e)))?;

    // Extract metrics
    let metrics = extract_metrics(&font_face, size_lpx);

    // Build font handle from COM pointer + size + scale
    let ptr = font_face.as_raw() as *const u8;
    let handle = FontHandle::from_ptr_size_scale(ptr, size_lpx, scale);

    Ok(DirectWriteFont {
        font_face,
        em_size_dip: size_lpx, // 1 lpx = 1 DIP
        pixels_per_dip: scale,
        size_lpx,
        scale,
        metrics,
        handle,
    })
}

/// Extract font metrics from a DirectWrite font face.
pub fn extract_metrics(font_face: &IDWriteFontFace, em_size_dip: f32) -> FontMetrics {
    let mut dw_metrics = Default::default();
    unsafe { font_face.GetMetrics(&mut dw_metrics) };

    let units_per_em = dw_metrics.designUnitsPerEm as f32;
    let units_to_lpx = em_size_dip / units_per_em;

    FontMetrics {
        ascent_lpx: dw_metrics.ascent as f32 * units_to_lpx,
        descent_lpx: -(dw_metrics.descent as f32 * units_to_lpx),
        line_gap_lpx: dw_metrics.lineGap as f32 * units_to_lpx,
        x_height_lpx: dw_metrics.xHeight as f32 * units_to_lpx,
        cap_height_lpx: dw_metrics.capHeight as f32 * units_to_lpx,
        units_per_em: dw_metrics.designUnitsPerEm as u32,
    }
}
