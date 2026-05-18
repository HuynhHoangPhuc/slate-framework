//! Text run builder — converts shaped glyphs to GPU instances.

use slate_renderer::atlas::Atlas;
use slate_renderer::scene::GlyphInstance;

use crate::TextError;
use crate::backend::{Font, TextBackend};
use crate::font_handle::FontHandle;
use crate::glyph_cache::GlyphCache;
use crate::types::ShapedLine;

/// Builds `GlyphInstance`s from shaped text for GPU rendering.
///
/// Uses lazy per-variant rasterization: computes sub-pixel variant from screen
/// position, then rasterizes only the needed variant on demand.
///
/// Coordinates are accumulated in logical pixels (lpx); physical conversion
/// happens once at the end to avoid precision loss.
pub struct TextRunBuilder<'a, B: TextBackend> {
    /// Text backend (for rasterization and bounds queries).
    pub backend: &'a B,
    /// Font used for rendering.
    pub font: &'a B::Font,
    /// Baseline origin in logical pixels `[x, y]`.
    pub baseline_lpx: [f32; 2],
    /// Text color (premultiplied RGBA).
    pub color: [f32; 4],
}

impl<'a, B: TextBackend> TextRunBuilder<'a, B> {
    /// Builds GPU glyph instances from shaped text with immediate rasterization.
    ///
    /// Computes sub-pixel variant from screen position and rasterizes missing
    /// glyphs on demand, uploading immediately to the atlas. This ensures glyphs
    /// are available in cache within the same frame.
    ///
    /// Whitespace glyphs are skipped efficiently using bounds-check-before-rasterize.
    ///
    /// # Arguments
    ///
    /// * `shaped` - Shaped line of text
    /// * `cache` - Glyph cache (mutated to rasterize missing glyphs)
    /// * `atlas` - GPU atlas for glyph storage
    /// * `queue` - GPU queue for upload commands
    pub fn build(
        &self,
        shaped: &ShapedLine,
        cache: &mut GlyphCache,
        atlas: &mut Atlas,
        queue: &wgpu::Queue,
    ) -> Result<Vec<GlyphInstance>, TextError> {
        self.build_line_at(shaped, cache, atlas, queue, 0.0)
    }

    /// Builds GPU glyph instances from a paragraph (multiple lines).
    ///
    /// Each line's `y_offset_lpx` is added to the baseline Y position.
    /// Use with `shape_paragraph()` results.
    ///
    /// # Arguments
    ///
    /// * `lines` - Shaped lines from `shape_paragraph()`
    /// * `cache` - Glyph cache (mutated to rasterize missing glyphs)
    /// * `atlas` - GPU atlas for glyph storage
    /// * `queue` - GPU queue for upload commands
    pub fn build_paragraph(
        &self,
        lines: &[ShapedLine],
        cache: &mut GlyphCache,
        atlas: &mut Atlas,
        queue: &wgpu::Queue,
    ) -> Result<Vec<GlyphInstance>, TextError> {
        let mut out = Vec::new();
        for line in lines {
            let instances = self.build_line_at(line, cache, atlas, queue, line.y_offset_lpx)?;
            out.extend(instances);
        }
        Ok(out)
    }

    /// Builds GPU glyph instances for a single line at a given Y offset.
    ///
    /// Dispatches per-glyph on `ShapedGlyph::font_handle`: glyphs that the
    /// platform shaper rendered with a substitute face go through
    /// `backend.font_for(handle) → rasterize_glyph`, so CJK/emoji glyphs
    /// produced by DirectWrite's or CoreText's internal fallback don't get
    /// mis-rasterized against the primary face. The default sentinel handle
    /// (or a handle matching the primary) takes the original primary path.
    fn build_line_at(
        &self,
        shaped: &ShapedLine,
        cache: &mut GlyphCache,
        atlas: &mut Atlas,
        queue: &wgpu::Queue,
        y_offset_lpx: f32,
    ) -> Result<Vec<GlyphInstance>, TextError> {
        let scale = self.font.scale();
        let primary_h = self.font.handle();
        let mut out = Vec::with_capacity(shaped.glyphs.len());
        let mut pen_x_lpx = self.baseline_lpx[0];

        for g in &shaped.glyphs {
            let glyph_x_lpx = pen_x_lpx + g.x_offset_lpx;
            let glyph_x_px = glyph_x_lpx * scale;
            let variant = compute_variant(glyph_x_px);

            // Sentinel default → use primary; otherwise dispatch on captured face.
            let use_primary =
                g.font_handle == FontHandle::default() || g.font_handle == primary_h;

            let (bounds, key_handle) = if use_primary {
                (
                    self.backend.glyph_raster_bounds(self.font, g.glyph_id)?,
                    primary_h,
                )
            } else if let Some(sub_font) = self.backend.font_for(g.font_handle) {
                (
                    self.backend.glyph_raster_bounds(sub_font, g.glyph_id)?,
                    g.font_handle,
                )
            } else {
                // Unknown handle — fall back to primary so we don't drop the
                // glyph silently. This is a defensive path; the registry
                // should always know any handle that flowed through shaping.
                (
                    self.backend.glyph_raster_bounds(self.font, g.glyph_id)?,
                    primary_h,
                )
            };

            if bounds.is_whitespace() {
                pen_x_lpx += g.x_advance_lpx;
                continue;
            }

            if use_primary || key_handle == primary_h {
                cache.materialize(
                    self.backend,
                    self.font,
                    g.glyph_id,
                    variant,
                    atlas,
                    queue,
                )?;
            } else {
                cache.materialize_by_handle(
                    self.backend,
                    key_handle,
                    g.glyph_id,
                    variant,
                    atlas,
                    queue,
                )?;
            }

            if let Some(cg) = cache.get(key_handle, g.glyph_id, variant) {
                let origin_x_px = (glyph_x_lpx + cg.metrics.bearing_x_lpx) * scale;
                let origin_y_px =
                    (self.baseline_lpx[1] + y_offset_lpx - cg.metrics.bearing_y_lpx) * scale;

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
