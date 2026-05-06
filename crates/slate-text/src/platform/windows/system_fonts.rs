//! System font enumeration for Windows via DirectWrite.

use crate::error::TextError;
use crate::types::{FontDescriptor, FontStyle};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_OBLIQUE, IDWriteFactory5, IDWriteFontCollection1,
    IDWriteLocalizedStrings,
};

/// Enumerate all system fonts, returning metadata without loading full fonts.
pub fn enumerate_system_fonts(factory: &IDWriteFactory5) -> Result<Vec<FontDescriptor>, TextError> {
    let mut collection: Option<IDWriteFontCollection1> = None;
    unsafe { factory.GetSystemFontCollection(false, &mut collection, false) }.map_err(|e| {
        TextError::SystemFontEnumeration(format!("GetSystemFontCollection failed: {}", e))
    })?;
    let collection = collection.ok_or_else(|| {
        TextError::SystemFontEnumeration("GetSystemFontCollection returned None".into())
    })?;

    let family_count = unsafe { collection.GetFontFamilyCount() };
    let mut result = Vec::new();

    for i in 0..family_count {
        let family = match unsafe { collection.GetFontFamily(i) } {
            Ok(f) => f,
            Err(_) => continue,
        };

        // Get family name
        let family_names: IDWriteLocalizedStrings = match unsafe { family.GetFamilyNames() } {
            Ok(n) => n,
            Err(_) => continue,
        };

        let family_name = match get_localized_string(&family_names) {
            Some(n) => n,
            None => continue,
        };

        let font_count = unsafe { family.GetFontCount() };

        for j in 0..font_count {
            let font = match unsafe { family.GetFont(j) } {
                Ok(f) => f,
                Err(_) => continue,
            };

            // Get weight (100-900)
            let weight = unsafe { font.GetWeight() }.0 as u16;

            // Get style
            let dw_style = unsafe { font.GetStyle() };
            let is_italic =
                dw_style == DWRITE_FONT_STYLE_ITALIC || dw_style == DWRITE_FONT_STYLE_OBLIQUE;

            let style = match (weight >= 700, is_italic) {
                (true, true) => FontStyle::BoldItalic,
                (true, false) => FontStyle::Bold,
                (false, true) => FontStyle::Italic,
                (false, false) => FontStyle::Regular,
            };

            // Get face names for PostScript name approximation
            let face_names: IDWriteLocalizedStrings = match unsafe { font.GetFaceNames() } {
                Ok(n) => n,
                Err(_) => continue,
            };

            let face_name = get_localized_string(&face_names).unwrap_or_default();
            let postscript_name = if face_name.is_empty() {
                family_name.clone()
            } else {
                format!(
                    "{}-{}",
                    family_name.replace(' ', ""),
                    face_name.replace(' ', "")
                )
            };

            result.push(FontDescriptor {
                family: family_name.clone(),
                postscript_name,
                weight,
                style,
                path: None, // DirectWrite doesn't expose font paths directly
            });
        }
    }

    Ok(result)
}

/// Extract the first localized string (preferring en-US).
fn get_localized_string(strings: &IDWriteLocalizedStrings) -> Option<String> {
    let count = unsafe { strings.GetCount() };
    if count == 0 {
        return None;
    }

    // Try to find en-us locale
    let mut index = 0u32;
    let mut exists = false.into();
    let locale = windows::core::w!("en-us");

    let _ = unsafe { strings.FindLocaleName(locale, &mut index, &mut exists) };

    if !exists.as_bool() {
        index = 0; // Fall back to first string
    }

    // Get string length
    let len = match unsafe { strings.GetStringLength(index) } {
        Ok(l) => l as usize,
        Err(_) => return None,
    };

    // Get string
    let mut buffer = vec![0u16; len + 1];
    if unsafe { strings.GetString(index, &mut buffer) }.is_err() {
        return None;
    }

    // Find null terminator and convert
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16(&buffer[..end]).ok()
}
