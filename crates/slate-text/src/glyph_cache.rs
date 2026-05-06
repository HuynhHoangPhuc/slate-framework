//! Glyph cache for atlas-backed text rendering.
//!
//! `GlyphCache` rasterizes shaped glyphs and uploads them to the GPU atlas.
//! Cache key is `(FontHandle, glyph_id, variant)`.

use std::collections::HashMap;

use slate_renderer::atlas::{Atlas, AtlasAllocation};
use slate_renderer::glyph_pipeline::allocate_glyph;

use crate::backend::{Font, TextBackend};
use crate::error::TextError;
use crate::font_handle::FontHandle;
use crate::types::{CachedGlyph, GlyphBitmap, GlyphMetrics};

/// Cache of rasterized glyphs backed by a GPU atlas.
///
/// # Usage
///
/// Call `materialize()` to rasterize and upload glyphs on demand,
/// then `get()` to retrieve cached glyphs for rendering.
///
/// # Cache Key
///
/// Glyphs are keyed by `(FontHandle, glyph_id, variant)` where:
/// - `FontHandle` encodes font pointer, size, and scale
/// - `glyph_id` is the glyph index in the font
/// - `variant` is the sub-pixel X offset (0-3)
pub struct GlyphCache {
    cache: HashMap<(FontHandle, u32, u8), CachedGlyph>,
}

impl GlyphCache {
    /// Creates a new empty glyph cache.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    /// Looks up a cached glyph by font handle, glyph ID, and variant.
    pub fn get(&self, font: FontHandle, glyph_id: u32, variant: u8) -> Option<&CachedGlyph> {
        self.cache.get(&(font, glyph_id, variant))
    }

    /// Rasterizes and uploads a single glyph variant to the atlas.
    ///
    /// Makes the glyph available in cache for subsequent `get()` calls.
    ///
    /// Returns `true` if rasterized (cache miss), `false` if already cached.
    pub fn materialize<B: TextBackend>(
        &mut self,
        backend: &B,
        font: &B::Font,
        glyph_id: u32,
        variant: u8,
        atlas: &mut Atlas,
        queue: &wgpu::Queue,
    ) -> Result<bool, TextError> {
        let key = (font.handle(), glyph_id, variant);

        if self.cache.contains_key(&key) {
            return Ok(false);
        }

        let bitmap = backend.rasterize_glyph(font, glyph_id, variant)?;
        if bitmap.width == 0 || bitmap.height == 0 {
            return Ok(false);
        }

        let (alloc_id, uv_rect) = allocate_glyph(atlas, bitmap.width, bitmap.height)
            .map_err(|e| TextError::RasterizationFailed(format!("atlas alloc: {e:?}")))?;

        let padded = pad_with_gutter(&bitmap.alpha, bitmap.width, bitmap.height);
        atlas.upload(queue, alloc_id, &padded);

        let metrics = GlyphMetrics::from_bitmap(&bitmap);
        self.cache.insert(
            key,
            CachedGlyph {
                alloc: AtlasAllocation { uv_rect, alloc_id },
                metrics,
            },
        );

        Ok(true)
    }

    /// Marks a glyph as recently used for LRU tracking.
    ///
    /// Call this for each glyph rendered to prevent atlas eviction.
    pub fn touch(&mut self, atlas: &mut Atlas, font: FontHandle, glyph_id: u32, variant: u8) {
        if let Some(cg) = self.cache.get(&(font, glyph_id, variant)) {
            atlas.touch(cg.alloc.alloc_id);
        }
    }

    /// Clears all cached glyphs.
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    /// Returns the number of cached glyphs.
    ///
    /// Useful for testing and debugging.
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }
}

impl Default for GlyphCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Pads a glyph bitmap with a 1-pixel zero-fill gutter on all sides.
///
/// Input: `src` is `w × h` R8 row-major, no padding.
/// Output: `(w+2) × (h+2)` with zero border, inner `w × h` copied from src.
pub fn pad_with_gutter(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    let pw = (w + 2) as usize;
    let ph = (h + 2) as usize;
    let mut buf = vec![0u8; pw * ph];
    for y in 0..h as usize {
        let src_off = y * w as usize;
        let dst_off = (y + 1) * pw + 1;
        buf[dst_off..dst_off + w as usize].copy_from_slice(&src[src_off..src_off + w as usize]);
    }
    buf
}

impl GlyphMetrics {
    /// Creates metrics from a glyph bitmap.
    pub fn from_bitmap(bitmap: &GlyphBitmap) -> Self {
        Self {
            width: bitmap.width,
            height: bitmap.height,
            bearing_x_lpx: bitmap.bearing_x_lpx,
            bearing_y_lpx: bitmap.bearing_y_lpx,
            advance_x_lpx: bitmap.advance_x_lpx,
        }
    }
}
