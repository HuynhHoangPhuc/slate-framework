//! Text run builder — converts shaped glyphs to GPU instances.

use slate_renderer::scene::GlyphInstance;

use crate::TextError;
use crate::backend::{Font, TextBackend};
use crate::glyph_cache::GlyphCache;
use crate::types::ShapedLine;

/// Builds `GlyphInstance`s from shaped text for GPU rendering.
///
/// Coordinates are accumulated in logical pixels (lpx); physical conversion
/// happens once at the end to avoid precision loss.
pub struct TextRunBuilder<'a, B: TextBackend> {
    /// Text backend (for scale factor).
    pub backend: &'a B,
    /// Font used for rendering.
    pub font: &'a B::Font,
    /// Glyph cache with pre-rasterized glyphs.
    pub cache: &'a GlyphCache,
    /// Baseline origin in logical pixels `[x, y]`.
    pub baseline_lpx: [f32; 2],
    /// Text color (premultiplied RGBA).
    pub color: [f32; 4],
}

impl<'a, B: TextBackend> TextRunBuilder<'a, B> {
    /// Builds GPU glyph instances from shaped text.
    ///
    /// Cache misses (whitespace or un-rasterized glyphs) are skipped silently.
    /// The glyph's advance still contributes to pen position, but no quad is
    /// generated. This handles whitespace correctly but cannot distinguish
    /// expected whitespace from bugs where visible glyphs weren't materialized.
    ///
    /// Phase 2b will adopt Zed's bounds-check-before-rasterize pattern to fix
    /// this limitation. See `plans/reports/decision-260506-1020-zed-style-bounds-check-glyph-rasterization.md`.
    pub fn build(&self, shaped: &ShapedLine) -> Result<Vec<GlyphInstance>, TextError> {
        let scale = self.font.scale();
        let mut out = Vec::with_capacity(shaped.glyphs.len());
        let mut pen_x_lpx = self.baseline_lpx[0];

        for g in &shaped.glyphs {
            let glyph_x_lpx = pen_x_lpx + g.x_offset_lpx;
            let glyph_x_px = glyph_x_lpx * scale;
            let variant = compute_variant(glyph_x_px);

            // Cache miss = whitespace or empty glyph, skip (advance only, no quad)
            if let Some(cg) = self.cache.get(self.font.handle(), g.glyph_id, variant) {
                let origin_x_px = (glyph_x_lpx + cg.metrics.bearing_x_lpx) * scale;
                let origin_y_px = (self.baseline_lpx[1] - cg.metrics.bearing_y_lpx) * scale;

                out.push(GlyphInstance {
                    rect: [
                        origin_x_px.floor(),
                        origin_y_px.round(),
                        cg.metrics.width as f32,
                        cg.metrics.height as f32,
                    ],
                    uv_rect: cg.alloc.uv_rect,
                    color: self.color,
                    sub_pixel_variant: variant as u32,
                    _pad: [0; 3],
                });
            }

            pen_x_lpx += g.x_advance_lpx;
        }

        Ok(out)
    }
}

/// Computes sub-pixel variant (0-3) from physical X position.
///
/// Uses `rem_euclid` to handle negative offsets correctly.
fn compute_variant(x_px: f32) -> u8 {
    let frac = x_px.rem_euclid(1.0);
    ((frac * 4.0).round() as u32 % 4) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_positive_values() {
        assert_eq!(compute_variant(0.0), 0);
        assert_eq!(compute_variant(0.125), 1); // 0.125 * 4 = 0.5 → rounds to 0, then % 4
        assert_eq!(compute_variant(0.25), 1);
        assert_eq!(compute_variant(0.5), 2);
        assert_eq!(compute_variant(0.75), 3);
        assert_eq!(compute_variant(1.0), 0);
        assert_eq!(compute_variant(100.25), 1);
    }

    #[test]
    fn variant_negative_values() {
        // rem_euclid ensures negative values map to [0, 1)
        assert_eq!(compute_variant(-0.0), 0);
        assert_eq!(compute_variant(-0.25), 3); // -0.25.rem_euclid(1.0) = 0.75
        assert_eq!(compute_variant(-1e-7), 0); // Tiny negative should not wrap
        assert_eq!(compute_variant(-0.5), 2);
    }

    #[test]
    fn variant_boundary() {
        // Edge case: 100.0 - tiny epsilon should still be variant 0
        let x = 100.0 - 1e-7;
        assert_eq!(compute_variant(x), 0);
        assert_eq!(compute_variant(100.0), 0);
    }
}
