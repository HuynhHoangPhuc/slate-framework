//! System font enumeration for macOS via CoreText.

use crate::error::TextError;
use crate::types::{FontDescriptor, FontStyle};
use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_text::{
    CTFontCollectionCreateFromAvailableFonts, CTFontCollectionCreateMatchingFontDescriptors,
    CTFontDescriptorCopyAttribute, kCTFontFamilyNameAttribute, kCTFontNameAttribute,
    kCTFontSlantTrait, kCTFontTraitsAttribute, kCTFontURLAttribute, kCTFontWeightTrait,
};
use std::path::PathBuf;

/// Enumerate all system fonts, returning metadata without loading full fonts.
pub fn enumerate_system_fonts() -> Result<Vec<FontDescriptor>, TextError> {
    let collection = unsafe { CTFontCollectionCreateFromAvailableFonts(std::ptr::null()) };

    let descriptors: CFRetained<CFArray<objc2_core_text::CTFontDescriptor>> =
        unsafe { CTFontCollectionCreateMatchingFontDescriptors(collection.as_ref()) }.ok_or_else(
            || TextError::SystemFontEnumeration("Failed to get font descriptors".into()),
        )?;

    let mut result = Vec::new();
    let count = descriptors.len();

    for i in 0..count {
        let desc = &descriptors[i];

        // Extract family name
        let family = unsafe {
            CTFontDescriptorCopyAttribute(desc, kCTFontFamilyNameAttribute)
                .map(|attr| attr.downcast::<CFString>())
        }
        .flatten()
        .map(|s| s.to_string())
        .unwrap_or_default();

        if family.is_empty() {
            continue;
        }

        // Extract PostScript name
        let postscript_name = unsafe {
            CTFontDescriptorCopyAttribute(desc, kCTFontNameAttribute)
                .map(|attr| attr.downcast::<CFString>())
        }
        .flatten()
        .map(|s| s.to_string())
        .unwrap_or_else(|| family.clone());

        // Extract traits dict for weight and slant
        let (weight, is_italic) = unsafe {
            CTFontDescriptorCopyAttribute(desc, kCTFontTraitsAttribute)
                .map(|attr| attr.downcast::<CFDictionary<CFString, CFType>>())
        }
        .flatten()
        .map(|traits| extract_weight_and_slant(&traits))
        .unwrap_or((400, false));

        // Derive style from weight and italic
        let style = match (weight >= 700, is_italic) {
            (true, true) => FontStyle::BoldItalic,
            (true, false) => FontStyle::Bold,
            (false, true) => FontStyle::Italic,
            (false, false) => FontStyle::Regular,
        };

        // Extract font file URL
        let path = unsafe {
            CTFontDescriptorCopyAttribute(desc, kCTFontURLAttribute)
                .map(|attr| attr.downcast::<objc2_core_foundation::CFURL>())
        }
        .flatten()
        .and_then(|url| url.to_path());

        result.push(FontDescriptor {
            family,
            postscript_name,
            weight,
            style,
            path,
        });
    }

    Ok(result)
}

/// Extract weight (100-900) and italic flag from traits dictionary.
fn extract_weight_and_slant(traits: &CFDictionary<CFString, CFType>) -> (u16, bool) {
    // CoreText weight is -1.0 to 1.0 (0.0 = regular, 0.4 = bold)
    let weight_ct = unsafe {
        traits
            .get(kCTFontWeightTrait)
            .and_then(|v| v.downcast_ref::<CFNumber>())
            .and_then(|n| n.to_f64())
    }
    .unwrap_or(0.0);

    // Convert CoreText weight to CSS weight (100-900)
    // CT -1.0 → 100, CT 0.0 → 400, CT 0.4 → 700, CT 1.0 → 900
    let weight_css = ((weight_ct + 1.0) * 400.0).clamp(100.0, 900.0) as u16;

    // Slant > 0 indicates italic
    let slant = unsafe {
        traits
            .get(kCTFontSlantTrait)
            .and_then(|v| v.downcast_ref::<CFNumber>())
            .and_then(|n| n.to_f64())
    }
    .unwrap_or(0.0);

    (weight_css, slant > 0.05)
}
