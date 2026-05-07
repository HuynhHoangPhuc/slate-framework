//! Font loading for CoreText backend.
//!
//! Loads fonts from static byte slices using `CTFontManagerCreateFontDescriptorFromData`.
//! System font lookup is deferred to Phase 2b.

use crate::error::TextError;
use crate::types::FontMetrics;
use objc2_core_foundation::{CFData, CFRetained, CGFloat};
use objc2_core_text::{CTFont, CTFontManagerCreateFontDescriptorFromData};
use std::ptr;

use super::PT_TO_LPX;

/// Logical pixels to CoreText points conversion factor.
/// 1 lpx = 1/96 inch, 1 pt = 1/72 inch, so pt = lpx * 72/96.
const LPX_TO_PT: f64 = 72.0 / 96.0;

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

    // Convert logical pixels to CoreText points
    let ct_size_pt = (size_lpx as f64) * LPX_TO_PT;

    // Create the font at the specified size
    let ct_font =
        unsafe { CTFont::with_font_descriptor(&descriptor, ct_size_pt as CGFloat, ptr::null()) };

    Ok((ct_font, data))
}

/// Extract font metrics from a CTFont, converting from points to logical pixels.
pub fn extract_metrics(ct_font: &CTFont) -> FontMetrics {
    // CT returns metrics in points at the font's current size
    let ascent_pt = unsafe { ct_font.ascent() } as f32;
    let descent_pt = unsafe { ct_font.descent() } as f32;
    let leading_pt = unsafe { ct_font.leading() } as f32;
    let x_height_pt = unsafe { ct_font.x_height() } as f32;
    let cap_height_pt = unsafe { ct_font.cap_height() } as f32;
    let units_per_em = unsafe { ct_font.units_per_em() };

    FontMetrics {
        ascent_lpx: ascent_pt * PT_TO_LPX,
        descent_lpx: -descent_pt * PT_TO_LPX, // CT returns positive; we want negative
        line_gap_lpx: leading_pt * PT_TO_LPX,
        x_height_lpx: x_height_pt * PT_TO_LPX,
        cap_height_lpx: cap_height_pt * PT_TO_LPX,
        units_per_em,
    }
}
