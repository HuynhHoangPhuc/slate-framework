//! Font loading for CoreText backend.
//!
//! Loads fonts from static byte slices using `CTFontManagerCreateFontDescriptorFromData`.
//! System font lookup is deferred to Phase 2b.

use crate::error::TextError;
use crate::types::FontMetrics;
use objc2_core_foundation::{CFData, CFRetained, CGFloat};
use objc2_core_text::{CTFont, CTFontManagerCreateFontDescriptorFromData};
use std::ptr;

/// Create a CTFont from raw TTF/OTF bytes.
///
/// # Note on Data Lifetime
///
/// `CFData::from_bytes` copies the data into a CF-managed buffer. The `'static` bound
/// ensures the source slice is stable, and the returned `CFRetained<CFData>` owns the
/// copy. The CFData must be stored alongside the CTFont because the font descriptor
/// holds a reference to it.
///
/// # Arguments
///
/// * `bytes` - Raw font file data (TTF/OTF format, must be 'static)
/// * `size_lpx` - Font size in logical pixels
///
/// # Returns
///
/// Tuple of (CTFont, CFData) - caller must store both to keep the font alive.
pub fn create_font_from_bytes(
    bytes: &'static [u8],
    size_lpx: f32,
) -> Result<(CFRetained<CTFont>, CFRetained<CFData>), TextError> {
    // Create CFData wrapping the static bytes (no copy, no deallocation)
    let data = CFData::from_bytes(bytes);

    // Create font descriptor from the data
    let descriptor =
        unsafe { CTFontManagerCreateFontDescriptorFromData(&data) }.ok_or_else(|| {
            TextError::FontFileLoad(
                "CTFontManagerCreateFontDescriptorFromData returned null".into(),
            )
        })?;

    // AppKit NSView convention: 1 pt = 1 logical pixel. CTFont takes its size
    // in points; we pass `size_lpx` through unchanged so glyph proportions
    // match every other macOS-native text surface.
    let ct_size_pt = size_lpx as f64;

    // Create the font at the specified size
    let ct_font =
        unsafe { CTFont::with_font_descriptor(&descriptor, ct_size_pt as CGFloat, ptr::null()) };

    Ok((ct_font, data))
}

/// Extract font metrics from a CTFont. With `1 pt = 1 lpx` the CT-returned
/// point values are already in lpx.
pub fn extract_metrics(ct_font: &CTFont) -> FontMetrics {
    let ascent = unsafe { ct_font.ascent() } as f32;
    let descent = unsafe { ct_font.descent() } as f32;
    let leading = unsafe { ct_font.leading() } as f32;
    let x_height = unsafe { ct_font.x_height() } as f32;
    let cap_height = unsafe { ct_font.cap_height() } as f32;
    let units_per_em = unsafe { ct_font.units_per_em() };

    FontMetrics {
        ascent_lpx: ascent,
        descent_lpx: -descent, // CT returns positive; we want negative
        line_gap_lpx: leading,
        x_height_lpx: x_height,
        cap_height_lpx: cap_height,
        units_per_em,
    }
}
