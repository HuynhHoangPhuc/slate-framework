//! System font enumeration for macOS via CoreText.

use crate::error::TextError;
use crate::types::{FontDescriptor, FontStyle};
use objc2_core_foundation::{CFDictionary, CFIndex, CFNumber, CFString};
use objc2_core_text::{
    CTFontCollection, CTFontDescriptor, kCTFontFamilyNameAttribute, kCTFontNameAttribute,
    kCTFontSlantTrait, kCTFontTraitsAttribute, kCTFontURLAttribute, kCTFontWeightTrait,
};
use std::ffi::c_void;

/// Enumerate all system fonts, returning metadata without loading full fonts.
pub fn enumerate_system_fonts() -> Result<Vec<FontDescriptor>, TextError> {
    let collection = unsafe { CTFontCollection::from_available_fonts(None) };

    let descriptors = unsafe { collection.matching_font_descriptors() }
        .ok_or_else(|| TextError::SystemFontEnumeration("Failed to get font descriptors".into()))?;

    let mut result = Vec::new();
    let count = descriptors.len();

    for i in 0..count {
        // Get descriptor from untyped array
        let desc_ptr = unsafe { descriptors.value_at_index(i as CFIndex) };
        if desc_ptr.is_null() {
            continue;
        }
        let desc: &CTFontDescriptor = unsafe { &*(desc_ptr as *const CTFontDescriptor) };

        // Extract family name
        let family = unsafe { desc.attribute(kCTFontFamilyNameAttribute) }
            .and_then(|attr| attr.downcast::<CFString>().ok())
            .map(|s| s.to_string())
            .unwrap_or_default();

        if family.is_empty() {
            continue;
        }

        // Extract PostScript name
        let postscript_name = unsafe { desc.attribute(kCTFontNameAttribute) }
            .and_then(|attr| attr.downcast::<CFString>().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| family.clone());

        // Extract traits dict for weight and slant
        let (weight, is_italic) = unsafe { desc.attribute(kCTFontTraitsAttribute) }
            .and_then(|attr| attr.downcast::<CFDictionary>().ok())
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
        let path = unsafe { desc.attribute(kCTFontURLAttribute) }
            .and_then(|attr| attr.downcast::<objc2_core_foundation::CFURL>().ok())
            .and_then(|url| url.to_file_path());

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
fn extract_weight_and_slant(traits: &CFDictionary) -> (u16, bool) {
    // CoreText weight is -1.0 to 1.0 (0.0 = regular, 0.4 = bold)
    let weight_ct = unsafe {
        let key = kCTFontWeightTrait as *const _ as *const c_void;
        let value_ptr = traits.value(key);
        if value_ptr.is_null() {
            None
        } else {
            let cf_number: &CFNumber = &*(value_ptr as *const CFNumber);
            cf_number.as_f64()
        }
    }
    .unwrap_or(0.0);

    // Convert CoreText weight to CSS weight (100-900)
    // CT -1.0 → 100, CT 0.0 → 400, CT 0.4 → 700, CT 1.0 → 900
    let weight_css = ((weight_ct + 1.0) * 400.0).clamp(100.0f64, 900.0f64) as u16;

    // Slant > 0 indicates italic
    let slant = unsafe {
        let key = kCTFontSlantTrait as *const _ as *const c_void;
        let value_ptr = traits.value(key);
        if value_ptr.is_null() {
            None
        } else {
            let cf_number: &CFNumber = &*(value_ptr as *const CFNumber);
            cf_number.as_f64()
        }
    }
    .unwrap_or(0.0);

    (weight_css, slant > 0.05)
}
