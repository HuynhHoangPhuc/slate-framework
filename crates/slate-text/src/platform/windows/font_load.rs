//! Font loading for DirectWrite backend.
//!
//! Uses IDWriteInMemoryFontFileLoader (DirectWrite 5+) to load fonts from static byte slices.
//! System font lookup by family name is not yet implemented.

use crate::{FontHandle, FontMetrics, TextError};
use log::debug;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_FACE_TYPE, DWRITE_FONT_FILE_TYPE, DWRITE_FONT_SIMULATIONS_NONE,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_REGULAR,
    IDWriteFactory5, IDWriteFontCollection1, IDWriteFontFace, IDWriteFontFile,
    IDWriteInMemoryFontFileLoader, IDWriteTextFormat,
};
use windows::core::{BOOL, HSTRING};

use super::DirectWriteFont;

/// Extract font family name from raw font bytes using ttf-parser.
fn extract_family_name(bytes: &[u8]) -> Result<String, TextError> {
    let face = ttf_parser::Face::parse(bytes, 0)
        .map_err(|e| TextError::FontFileLoad(format!("ttf-parser: {e}")))?;
    for name in face.names() {
        let is_family_name = name.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY
            || name.name_id == ttf_parser::name_id::FAMILY;
        if is_family_name && let Some(family) = name.to_string() {
            return Ok(family);
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
    // Validate byte slice length fits in u32 before passing to DirectWrite FFI.
    let bytes_len_u32 = u32::try_from(bytes.len())
        .map_err(|_| TextError::FontFileLoad("Font byte slice exceeds 4 GiB limit".into()))?;

    // Create font file from memory using the in-memory loader
    let font_file: IDWriteFontFile = unsafe {
        loader.CreateInMemoryFontFileReference(
            factory,
            bytes.as_ptr() as *const _,
            bytes_len_u32,
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

    // Re-create font file ref for the builder (font_file was consumed by CreateFontFace).
    // bytes_len_u32 was validated above; reuse it here.
    let font_file_for_set: IDWriteFontFile = unsafe {
        loader.CreateInMemoryFontFileReference(
            factory,
            bytes.as_ptr() as *const _,
            bytes_len_u32,
            None,
        )
    }
    .map_err(|e| TextError::FontFileLoad(format!("CreateInMemoryFontFileReference (set): {e}")))?;

    unsafe { font_set_builder.AddFontFile(&font_file_for_set) }
        .map_err(|e| TextError::FontFileLoad(format!("AddFontFile: {e}")))?;

    let font_set = unsafe { font_set_builder.CreateFontSet() }
        .map_err(|e| TextError::FontFileLoad(format!("CreateFontSet: {e}")))?;

    let font_collection: IDWriteFontCollection1 = unsafe {
        factory.CreateFontCollectionFromFontSet(&font_set)
    }
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

    let handle = FontHandle::from_face_id(
        super::font_id::idwrite_font_face_id(&font_face),
        size_lpx,
        scale,
    );

    Ok(DirectWriteFont {
        font_face,
        em_size_dip: size_lpx, // 1 lpx = 1 DIP
        pixels_per_dip: scale,
        size_lpx,
        scale,
        metrics,
        text_format: Some(text_format),
        handle,
    })
}

/// Load a font from the system font collection by family name.
///
/// Looks up `family` (case-insensitive) in the OS font collection, picks the
/// Regular weight/style face, and builds an IDWriteTextFormat bound to the
/// system collection so DirectWrite can resolve the family by name during
/// shaping.
pub fn load_system_font(
    factory: &IDWriteFactory5,
    family: &str,
    size_lpx: f32,
    scale: f32,
) -> Result<super::DirectWriteFont, TextError> {
    let mut collection: Option<IDWriteFontCollection1> = None;
    unsafe { factory.GetSystemFontCollection(false, &mut collection, false) }
        .map_err(|e| TextError::FontFileLoad(format!("GetSystemFontCollection: {e}")))?;
    let collection = collection.ok_or_else(|| TextError::FontNotFound {
        family: family.to_string(),
    })?;

    let family_w = HSTRING::from(family);
    let mut index: u32 = 0;
    let mut exists: BOOL = false.into();
    unsafe { collection.FindFamilyName(&family_w, &mut index, &mut exists) }
        .map_err(|e| TextError::FontFileLoad(format!("FindFamilyName({family}): {e}")))?;
    if !exists.as_bool() {
        return Err(TextError::FontNotFound {
            family: family.to_string(),
        });
    }

    let dw_family = unsafe { collection.GetFontFamily(index) }
        .map_err(|e| TextError::FontFileLoad(format!("GetFontFamily: {e}")))?;
    let dw_font = unsafe {
        dw_family.GetFirstMatchingFont(
            DWRITE_FONT_WEIGHT_REGULAR,
            DWRITE_FONT_STRETCH_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
        )
    }
    .map_err(|e| TextError::FontFileLoad(format!("GetFirstMatchingFont: {e}")))?;

    let font_face: IDWriteFontFace = unsafe { dw_font.CreateFontFace() }
        .map_err(|e| TextError::FontFileLoad(format!("CreateFontFace: {e}")))?;

    let metrics = extract_metrics(&font_face, size_lpx);

    debug!(
        "DirectWrite system font '{family}' (size={size_lpx}): ascent={:.2} descent={:.2} upem={}",
        metrics.ascent_lpx, metrics.descent_lpx, metrics.units_per_em,
    );

    // text_format bound to the system collection (None = system). Shaping
    // through this format will resolve `family` against the system fonts so
    // DirectWrite picks the same face we just opened.
    let text_format: IDWriteTextFormat = unsafe {
        factory.CreateTextFormat(
            &family_w,
            None,
            DWRITE_FONT_WEIGHT_REGULAR,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size_lpx,
            &HSTRING::from("en-US"),
        )
    }
    .map_err(|e| TextError::FontFileLoad(format!("CreateTextFormat: {e}")))?;

    let handle = FontHandle::from_face_id(
        super::font_id::idwrite_font_face_id(&font_face),
        size_lpx,
        scale,
    );

    Ok(super::DirectWriteFont {
        font_face,
        em_size_dip: size_lpx,
        pixels_per_dip: scale,
        size_lpx,
        scale,
        metrics,
        text_format: Some(text_format),
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
