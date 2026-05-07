//! Font loading for DirectWrite backend.
//!
//! Uses IDWriteInMemoryFontFileLoader (DirectWrite 5+) to load fonts from static byte slices.
//! System font lookup deferred to Phase 2b.

use crate::{FontHandle, FontMetrics, TextError};
use log::debug;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_FACE_TYPE, DWRITE_FONT_FILE_TYPE, DWRITE_FONT_SIMULATIONS_NONE,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_REGULAR,
    IDWriteFactory5, IDWriteFontFace, IDWriteFontFile, IDWriteFontCollection1,
    IDWriteInMemoryFontFileLoader, IDWriteTextFormat,
};
use windows::core::{Interface, HSTRING};

use super::DirectWriteFont;

/// Extract font family name from raw font bytes using ttf-parser.
fn extract_family_name(bytes: &[u8]) -> Result<String, TextError> {
    let face = ttf_parser::Face::parse(bytes, 0)
        .map_err(|e| TextError::FontFileLoad(format!("ttf-parser: {e}")))?;
    for name in face.names() {
        if name.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY
            || name.name_id == ttf_parser::name_id::FAMILY
        {
            if let Some(family) = name.to_string() {
                return Ok(family);
            }
        }
    }
    Err(TextError::FontFileLoad("No family name in font".into()))
}

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

    debug!(
        "DirectWrite font metrics (size={size_lpx}): ascent={:.2} descent={:.2} line_gap={:.2} x_height={:.2} cap_height={:.2} upem={}",
        metrics.ascent_lpx,
        metrics.descent_lpx,
        metrics.line_gap_lpx,
        metrics.x_height_lpx,
        metrics.cap_height_lpx,
        metrics.units_per_em,
    );

    // Extract real family name (empty name falls back to system default)
    let family_name = extract_family_name(bytes)?;

    // Build font set from the in-memory font file for IDWriteTextFormat
    let font_set_builder = unsafe { factory.CreateFontSetBuilder() }
        .map_err(|e| TextError::FontFileLoad(format!("CreateFontSetBuilder: {e}")))?;

    // Re-create font file ref for the builder (font_file was consumed by CreateFontFace)
    let font_file_for_set: IDWriteFontFile = unsafe {
        loader.CreateInMemoryFontFileReference(
            factory,
            bytes.as_ptr() as *const _,
            bytes.len() as u32,
            None,
        )
    }
    .map_err(|e| TextError::FontFileLoad(format!("CreateInMemoryFontFileReference (set): {e}")))?;

    unsafe { font_set_builder.AddFontFile(&font_file_for_set) }
        .map_err(|e| TextError::FontFileLoad(format!("AddFontFile: {e}")))?;

    let font_set = unsafe { font_set_builder.CreateFontSet() }
        .map_err(|e| TextError::FontFileLoad(format!("CreateFontSet: {e}")))?;

    let font_collection: IDWriteFontCollection1 =
        unsafe { factory.CreateFontCollectionFromFontSet(&font_set) }
            .map_err(|e| TextError::FontFileLoad(format!("CreateFontCollectionFromFontSet: {e}")))?;

    // Create IDWriteTextFormat with our custom collection + real family name
    let text_format: IDWriteTextFormat = unsafe {
        factory.CreateTextFormat(
            &HSTRING::from(&family_name),
            &font_collection,
            DWRITE_FONT_WEIGHT_REGULAR,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size_lpx,
            &HSTRING::from("en-US"),
        )
    }
    .map_err(|e| TextError::FontFileLoad(format!("CreateTextFormat: {e}")))?;

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
        text_format,
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
