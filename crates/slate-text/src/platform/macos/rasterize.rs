//! Glyph rasterization for CoreText backend.
//!
//! Rasterizes glyphs to alpha-only bitmaps using CGBitmapContext and CTFontDrawGlyphs.
//! Produces 4 sub-pixel X variants (0.0/0.25/0.5/0.75 pixel offsets).

use crate::error::TextError;
use crate::types::{GlyphBitmap, GlyphBounds};
use objc2_core_foundation::{CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGContext, CGImageAlphaInfo};
use objc2_core_text::{CTFont, CTFontOrientation};
use std::ptr::NonNull;

use super::PT_TO_LPX;

/// Maximum rasterization buffer dimension in pixels.
///
/// Prevents unbounded memory allocation for extremely large font sizes.
/// Glyphs requiring more than this many pixels per side are clamped,
/// which only affects fonts at ~256pt+ at 2x scale.
const MAX_RENDER_PX: usize = 512;

/// Rasterize a glyph to an alpha bitmap.
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

    // Compute render buffer size (2x em-square for safety with descenders/accents).
    // Capped at MAX_RENDER_PX to bound memory allocation for extreme font sizes.
    let uncapped = (size_lpx * scale * 2.0).ceil() as usize;
    if uncapped == 0 {
        return Err(TextError::RasterizationFailed("render size is zero".into()));
    }
    let render_size = if uncapped > MAX_RENDER_PX {
        log::warn!(
            "rasterize: render size {} exceeds MAX_RENDER_PX {}; clamping (font size {:.1}px, scale {:.2})",
            uncapped,
            MAX_RENDER_PX,
            size_lpx,
            scale,
        );
        MAX_RENDER_PX
    } else {
        uncapped
    };

    let render_w = render_size;
    let render_h = render_size;

    // Allocate alpha buffer
    let mut buffer: Vec<u8> = vec![0; render_w * render_h];

    // Create alpha-only bitmap context using CGBitmapContextCreate
    let ctx = unsafe {
        CGBitmapContextCreate(
            buffer.as_mut_ptr().cast(),
            render_w,
            render_h,
            8,        // bits per component
            render_w, // bytes per row
            None,     // colorspace (NULL for alpha-only)
            CGImageAlphaInfo::Only.0,
        )
    };

    let Some(ctx) = ctx else {
        return Err(TextError::RasterizationFailed(
            "CGBitmapContextCreate returned null".into(),
        ));
    };

    // Antialiasing: greyscale AA only (no LCD subpixel RGB triads).
    CGContext::set_should_antialias(Some(&*ctx), true);
    CGContext::set_should_smooth_fonts(Some(&*ctx), false);
    // Sub-pixel positioning ENABLED so the per-variant fractional shift in
    // `position` below produces actually-different AA bitmaps. Quantization
    // disabled so CG honors the exact requested fractional position instead
    // of snapping to integer device pixels.
    CGContext::set_allows_font_subpixel_positioning(Some(&*ctx), true);
    CGContext::set_should_subpixel_position_fonts(Some(&*ctx), true);
    CGContext::set_allows_font_subpixel_quantization(Some(&*ctx), false);
    CGContext::set_should_subpixel_quantize_fonts(Some(&*ctx), false);

    // CG bitmap context: bytes laid out top-down in memory (byte 0 =
    // top-left pixel) but user space is Y-up (origin bottom-left).
    // CTFont::draw_glyphs draws glyphs with ascenders extending in +Y user
    // space. We do NOT apply a Y-flip CTM — flipping user-Y would invert
    // the glyphs themselves. Drawing in CG-natural orientation produces
    // glyphs that land in the buffer's memory rows top-to-bottom (ascender
    // at low row indices, descender at high row indices) which matches the
    // GPU atlas's top-down sampling convention.
    //
    // Sub-pixel offset is passed via draw_glyphs `position` (user-space,
    // pre-scale), so we divide by `scale` to express it in device-pixel
    // quarter-units.
    let sub_pixel_offset_device: CGFloat = (variant as CGFloat) * 0.25;
    let sub_pixel_offset_user: CGFloat = sub_pixel_offset_device / scale as CGFloat;

    // Position glyph baseline in the buffer (device pixels, pre-scale CTM).
    let baseline_x: CGFloat = (render_w as CGFloat) * 0.25;
    let baseline_y: CGFloat = (render_h as CGFloat) * 0.25;

    // Move to baseline position (user-space; Y-up).
    CGContext::translate_ctm(Some(&*ctx), baseline_x, baseline_y);

    // Apply display scale.
    CGContext::scale_ctm(Some(&*ctx), scale as CGFloat, scale as CGFloat);

    // Draw the glyph at the sub-pixel-shifted position (in post-scale user
    // space, so multiplying by scale recovers device-pixel offset).
    let glyph = glyph_id;
    let position = CGPoint {
        x: sub_pixel_offset_user,
        y: 0.0,
    };

    unsafe {
        ct_font.draw_glyphs(NonNull::from(&glyph), NonNull::from(&position), 1, &ctx);
    }

    // Get advance width
    let mut advance = CGSize {
        width: 0.0,
        height: 0.0,
    };
    unsafe {
        ct_font.advances_for_glyphs(
            CTFontOrientation::Default,
            NonNull::from(&glyph),
            &mut advance as *mut CGSize,
            1,
        );
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
    // CG bitmap is top-down (row 0 = top), glyph origin at baseline_y from bottom
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

/// Query glyph raster bounds without rasterizing.
pub fn get_glyph_bounds(
    ct_font: &CTFont,
    glyph_id: u16,
    scale: f32,
) -> Result<GlyphBounds, TextError> {
    let mut bounds = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: 0.0,
            height: 0.0,
        },
    };

    unsafe {
        ct_font.bounding_rects_for_glyphs(
            CTFontOrientation::Default,
            NonNull::from(&glyph_id),
            &mut bounds as *mut CGRect,
            1,
        );
    }

    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return Ok(GlyphBounds::ZERO);
    }

    let width = (bounds.size.width as f32 * PT_TO_LPX * scale).ceil() as u32;
    let height = (bounds.size.height as f32 * PT_TO_LPX * scale).ceil() as u32;

    Ok(GlyphBounds { width, height })
}

/// Find tight bounding box of non-zero pixels in buffer.
///
/// Uses edge-inward scanning for early termination:
/// 1. Scan top→down to find `min_y` (stop on first non-zero row).
/// 2. Scan bottom→up to find `max_y`.
/// 3. Scan columns only within `min_y..=max_y` to find `min_x` / `max_x`.
///
/// This skips the outer zero-fill margin surrounding typical glyphs,
/// terminating far earlier than a full O(w×h) scan for normal glyph sizes.
fn find_tight_bounds(buffer: &[u8], width: usize, height: usize) -> (usize, usize, usize, usize) {
    // --- Step 1: min_y — scan rows top→down, stop on first hit ---
    let mut min_y = height; // sentinel: height means "not found"
    'top: for y in 0..height {
        for x in 0..width {
            if buffer[y * width + x] >= 1 {
                min_y = y;
                break 'top;
            }
        }
    }

    // Empty buffer — no non-zero pixels; return sentinels satisfying min > max
    if min_y == height {
        return (width, height, 0, 0);
    }

    // --- Step 2: max_y — scan rows bottom→up, stop on first hit ---
    let mut max_y = min_y;
    'bottom: for y in (min_y..height).rev() {
        for x in 0..width {
            if buffer[y * width + x] >= 1 {
                max_y = y;
                break 'bottom;
            }
        }
    }

    // --- Step 3: min_x / max_x — scan only within vertical bounds ---
    let mut min_x = width; // sentinel
    let mut max_x = 0usize;
    for y in min_y..=max_y {
        let row = &buffer[y * width..(y + 1) * width];
        // Narrow left bound: only scan left of current min_x
        for x in 0..min_x {
            if row[x] >= 1 {
                min_x = x;
                break;
            }
        }
        // Widen right bound: scan right→left, stop at first hit
        for x in (max_x..width).rev() {
            if row[x] >= 1 {
                if x > max_x {
                    max_x = x;
                }
                break;
            }
        }
    }

    (min_x, min_y, max_x, max_y)
}
