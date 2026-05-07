//! System font enumeration for Windows via DirectWrite.

use crate::error::TextError;
use crate::types::{FontDescriptor, FontStyle};
use std::path::PathBuf;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FONT_STYLE_ITALIC, DWRITE_FONT_STYLE_OBLIQUE, IDWriteFactory5, IDWriteFontCollection1,
    IDWriteFontFace, IDWriteLocalFontFileLoader, IDWriteLocalizedStrings,
};
use windows::core::Interface;

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

            // Extract the font file path via IDWriteFontFace + IDWriteLocalFontFileLoader.
            // Remote/cloud fonts won't have a local loader and will fall back to None.
            let path = extract_font_path(&font).ok().flatten();

            result.push(FontDescriptor {
                family: family_name.clone(),
                postscript_name,
                weight,
                style,
                path,
            });
        }
    }

    Ok(result)
}

/// Extract the local file path for a font via DirectWrite font face file references.
///
/// Steps:
/// 1. Create a font face from the IDWriteFont.
/// 2. Get its file references via GetFiles.
/// 3. Get the loader from the first file and QI to IDWriteLocalFontFileLoader.
/// 4. Get reference key from the file and call GetFilePathFromKey.
///
/// Returns None for remote/cloud fonts that use a non-local file loader.
fn extract_font_path(font: &windows::Win32::Graphics::DirectWrite::IDWriteFont) -> windows::core::Result<Option<PathBuf>> {
    // Step 1: Create font face
    let face: IDWriteFontFace = unsafe { font.CreateFontFace()? };

    // Step 2: Get file count
    let mut file_count = 0u32;
    unsafe { face.GetFiles(&mut file_count, None)? };
    if file_count == 0 {
        return Ok(None);
    }

    // Allocate storage and retrieve the file objects
    let mut font_files: Vec<Option<windows::Win32::Graphics::DirectWrite::IDWriteFontFile>> =
        (0..file_count).map(|_| None).collect();
    unsafe {
        face.GetFiles(&mut file_count, Some(font_files.as_mut_ptr()))?;
    }

    let font_file = match font_files.into_iter().flatten().next() {
        Some(f) => f,
        None => return Ok(None),
    };

    // Step 3: Get loader and QI to IDWriteLocalFontFileLoader
    let loader = unsafe { font_file.GetLoader()? };
    let local_loader: IDWriteLocalFontFileLoader = match loader.cast() {
        Ok(l) => l,
        // Not a local font (e.g. remote/cloud font) — fall back gracefully
        Err(_) => return Ok(None),
    };

    // Step 4: Get reference key and extract path
    let mut key_ptr: *mut core::ffi::c_void = core::ptr::null_mut();
    let mut key_size = 0u32;
    unsafe { font_file.GetReferenceKey(&mut key_ptr, &mut key_size)? };

    // GetFilePathLengthFromKey returns char count (without null terminator)
    let path_len = unsafe {
        local_loader.GetFilePathLengthFromKey(key_ptr as *const _, key_size)?
    };

    // Allocate buffer with room for null terminator
    let mut path_buf = vec![0u16; path_len as usize + 1];
    unsafe {
        local_loader.GetFilePathFromKey(key_ptr as *const _, key_size, &mut path_buf)?;
    }

    // Trim null terminator and convert to PathBuf
    let end = path_buf.iter().position(|&c| c == 0).unwrap_or(path_buf.len());
    let path_str = String::from_utf16(&path_buf[..end]).map_err(|_| {
        windows::core::Error::new(
            windows::core::HRESULT(-1i32),
            "Invalid UTF-16 in font path",
        )
    })?;

    Ok(Some(PathBuf::from(path_str)))
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
