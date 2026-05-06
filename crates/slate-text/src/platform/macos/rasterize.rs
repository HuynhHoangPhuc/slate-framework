//! Glyph rasterization for CoreText backend.
//!
//! Rasterizes glyphs to alpha-only bitmaps using CGBitmapContext and CTFontDrawGlyphs.
//! Produces 4 sub-pixel X variants (0.0/0.25/0.5/0.75 pixel offsets).

use crate::error::TextError;
use crate::types::GlyphBitmap;
use objc2_core_graphics::{
    CGAffineTransform, CGBitmapInfo, CGColorSpace, CGContext, CGFloat, CGPoint,
    kCGBitmapByteOrderDefault, kCGImageAlphaOnly,
};
use objc2_core_text::{CTFont, CTFontDrawGlyphs, CTFontGetAdvancesForGlyphs, kCTFontOrientationDefault};

use super::PT_TO_LPX;

/// Rasterize a glyph to an alpha bitmap.
///
/// # Arguments
///
/// * `ct_font` - CoreText font
/// * `glyph_id` - Glyph index in the font
/// * `size_lpx` - Font size in logical pixels
/// * `scale` - Display scale factor (e.g., 2.0 for Retina)
/// * `variant` - Sub-pixel X variant (0-3)
///
/// # Returns
///
/// GlyphBitmap with tight-cropped alpha data and metrics in logical pixels.
pub fn rasterize(
    ct_font: &CTFont,
    glyph_id: u16,
    size_lpx: f32,
    scale: f32,
    variant: u8,
) -> Result<GlyphBitmap, TextError> {
    if variant > 3 {
        return Err(TextError::RasterizationFailed(
            "variant out of range (must be 0-3)".into(),
        ));
    }

    // Compute render buffer size (2x em-square for safety with descenders/accents)
    let render_size = (size_lpx * scale * 2.0).ceil() as usize;
    if render_size == 0 {
        return Err(TextError::RasterizationFailed("render size is zero".into()));
    }

    let render_w = render_size;
    let render_h = render_size;

    // Allocate alpha buffer
    let mut buffer: Vec<u8> = vec![0; render_w * render_h];

    // Create alpha-only bitmap context
    let ctx = unsafe {
        CGContext::new(
            buffer.as_mut_ptr().cast(),
            render_w,
            render_h,
            8,                    // bits per component
            render_w,             // bytes per row
            None,                 // colorspace (NULL for alpha-only)
            kCGImageAlphaOnly.0 | kCGBitmapByteOrderDefault.0,
        )
    };

    let Some(ctx) = ctx else {
        return Err(TextError::RasterizationFailed(
            "CGBitmapContextCreate returned null".into(),
        ));
    };

    // Configure antialiasing (greyscale AA, no LCD subpixel)
    unsafe {
        ctx.set_should_antialias(true);
        ctx.set_should_smooth_fonts(false);
        ctx.set_allows_font_subpixel_positioning(false);
        ctx.set_should_subpixel_position_fonts(false);
        ctx.set_should_subpixel_quantize_fonts(false);
    }

    // Apply transforms:
    // 1. Flip Y (CoreGraphics has origin at bottom-left)
    // 2. Translate for sub-pixel variant
    // 3. Scale for display density
    let sub_pixel_offset = (variant as CGFloat) * 0.25;

    // Position glyph in center of buffer with room for bearings
    let baseline_x = (render_w as CGFloat) * 0.25;
    let baseline_y = (render_h as CGFloat) * 0.25;

    unsafe {
        // Flip Y coordinate system
        ctx.translate_ctm(0.0, render_h as CGFloat);
        ctx.scale_ctm(1.0, -1.0);

        // Move to baseline position
        ctx.translate_ctm(baseline_x + sub_pixel_offset, baseline_y);

        // Apply display scale
        ctx.scale_ctm(scale as CGFloat, scale as CGFloat);
    }

    // Draw the glyph
    let glyph = glyph_id;
    let position = CGPoint { x: 0.0, y: 0.0 };

    unsafe {
        CTFontDrawGlyphs(ct_font, &glyph, &position, 1, &ctx);
    }

    // Get advance width
    let mut advance = objc2_core_graphics::CGSize { width: 0.0, height: 0.0 };
    unsafe {
        CTFontGetAdvancesForGlyphs(ct_font, kCTFontOrientationDefault, &glyph, &mut advance, 1);
    }
    let advance_x_lpx = (advance.width as f32) * PT_TO_LPX;

    // Tight-crop: find bounding box of non-zero pixels
    let (min_x, min_y, max_x, max_y) = find_tight_bounds(&buffer, render_w, render_h);

    // Handle empty glyph (whitespace)
    if min_x > max_x || min_y > max_y {
        return Ok(GlyphBitmap {
            width: 0,
            height: 0,
            bearing_x_lpx: 0.0,
            bearing_y_lpx: 0.0,
            advance_x_lpx,
            alpha: vec![],
        });
    }

    let tight_w = max_x - min_x + 1;
    let tight_h = max_y - min_y + 1;

    // Extract cropped region
    let mut cropped = Vec::with_capacity(tight_w * tight_h);
    for y in min_y..=max_y {
        let row_start = y * render_w + min_x;
        cropped.extend_from_slice(&buffer[row_start..row_start + tight_w]);
    }

    // Compute bearings in logical pixels
    // bearing_x: distance from pen position to left edge of glyph
    // bearing_y: distance from baseline to top edge (positive up)
    let bearing_x_px = min_x as f32 - (baseline_x as f32);
    let bearing_y_px = (render_h as f32) - baseline_y as f32 - (min_y as f32);

    let bearing_x_lpx = bearing_x_px / scale;
    let bearing_y_lpx = bearing_y_px / scale;

    Ok(GlyphBitmap {
        width: tight_w as u32,
        height: tight_h as u32,
        bearing_x_lpx,
        bearing_y_lpx,
        advance_x_lpx,
        alpha: cropped,
    })
}

/// Find tight bounding box of non-zero pixels in buffer.
/// Returns (min_x, min_y, max_x, max_y). If no pixels found, returns invalid bounds.
fn find_tight_bounds(buffer: &[u8], width: usize, height: usize) -> (usize, usize, usize, usize) {
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;

    for y in 0..height {
        for x in 0..width {
            let idx = y * width + x;
            if buffer[idx] >= 1 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }

    (min_x, min_y, max_x, max_y)
}
