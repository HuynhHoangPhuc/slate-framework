//! Font smoothing dilation for light-on-dark text (macOS only).
//!
//! macOS CoreGraphics applies font smoothing that thins light text on dark backgrounds.
//! Dilation compensates by thickening the rasterized glyph.
//!
//! Windows DirectWrite ClearType handles light/dark text correctly without
//! compensation, so dilation is always 0 on Windows.

use crate::types::GlyphBounds;

/// Maximum dilation level (0-4).
pub const MAX_DILATION: u8 = 4;

/// Compute font smoothing dilation level from foreground color.
///
/// Light text on dark backgrounds appears thinner due to macOS font smoothing.
/// Dilation compensates by thickening the rasterized glyph.
///
/// Uses REC.601 luminance weights: R=0.30, G=0.59, B=0.11
///
/// # Arguments
///
/// * `rgb` - Foreground color as linear RGB values (0.0-1.0)
///
/// # Returns
///
/// Dilation level 0-4:
/// - 0: dark text (luminance > 0.75), no dilation needed
/// - 4: very light text (luminance < 0.25), maximum dilation
///
/// # Platform behavior
///
/// - macOS: Returns computed dilation (0-4)
/// - Windows: Always returns 0 (ClearType doesn't need dilation)
#[inline]
pub fn compute_dilation(rgb: [f32; 3]) -> u8 {
    #[cfg(target_os = "macos")]
    {
        let luminance = 0.30 * rgb[0] + 0.59 * rgb[1] + 0.11 * rgb[2];
        let dilation = 4.0 * (0.75 - luminance).max(0.0);
        dilation.min(MAX_DILATION as f32).round() as u8
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = rgb;
        0
    }
}

/// Apply dilation to glyph bounds.
///
/// Expands bounds by `dilation` pixels in each direction to accommodate
/// the thickened glyph.
///
/// # Arguments
///
/// * `bounds` - Original glyph bounds
/// * `dilation` - Dilation level (0-4)
///
/// # Returns
///
/// Expanded bounds if dilation > 0, otherwise original bounds.
#[inline]
pub fn dilate_bounds(bounds: GlyphBounds, dilation: u8) -> GlyphBounds {
    if dilation == 0 {
        return bounds;
    }
    let d = dilation as u32;
    GlyphBounds {
        width: bounds.width.saturating_add(2 * d),
        height: bounds.height.saturating_add(2 * d),
    }
}

/// Compute dilation from 8-bit sRGB color (common input format).
///
/// Converts sRGB to linear before computing luminance.
#[inline]
pub fn compute_dilation_srgb(rgb: [u8; 3]) -> u8 {
    // Approximate sRGB to linear conversion
    let r = (rgb[0] as f32 / 255.0).powf(2.2);
    let g = (rgb[1] as f32 / 255.0).powf(2.2);
    let b = (rgb[2] as f32 / 255.0).powf(2.2);
    compute_dilation([r, g, b])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn white_text_gets_no_dilation() {
        let dilation = compute_dilation([1.0, 1.0, 1.0]);
        #[cfg(target_os = "macos")]
        assert_eq!(dilation, 0, "White text has high luminance, no dilation");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(dilation, 0, "Non-macOS always returns 0");
    }

    #[test]
    fn black_text_gets_dilation() {
        let dilation = compute_dilation([0.0, 0.0, 0.0]);
        #[cfg(target_os = "macos")]
        assert_eq!(dilation, 3, "Black text gets dilation");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(dilation, 0, "Non-macOS always returns 0");
    }

    #[test]
    fn dilate_bounds_expands_correctly() {
        let bounds = GlyphBounds {
            width: 10,
            height: 12,
        };
        let dilated = dilate_bounds(bounds, 2);
        assert_eq!(dilated.width, 14);
        assert_eq!(dilated.height, 16);
    }

    #[test]
    fn dilate_bounds_zero_is_noop() {
        let bounds = GlyphBounds {
            width: 10,
            height: 12,
        };
        let dilated = dilate_bounds(bounds, 0);
        assert_eq!(dilated, bounds);
    }

    #[test]
    fn compute_dilation_srgb_converts_correctly() {
        // White in sRGB
        let dilation = compute_dilation_srgb([255, 255, 255]);
        #[cfg(target_os = "macos")]
        assert_eq!(dilation, 0);
        #[cfg(not(target_os = "macos"))]
        assert_eq!(dilation, 0);
    }
}
