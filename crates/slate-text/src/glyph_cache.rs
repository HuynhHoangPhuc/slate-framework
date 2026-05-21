//! Glyph cache for atlas-backed text rendering.
//!
//! `GlyphCache` rasterizes shaped glyphs and uploads them to the GPU atlas.
//! Cache key is `(FontHandle, glyph_id, variant)`.

use std::collections::HashMap;

use slate_renderer::atlas::Atlas;
use slate_renderer::glyph_pipeline::allocate_glyph;

use crate::backend::{Font, TextBackend};
use crate::error::TextError;
use crate::font_handle::FontHandle;
use crate::types::{CachedGlyph, GlyphBitmap, GlyphMetrics};

/// Default maximum number of entries in the glyph cache before eviction.
const DEFAULT_MAX_ENTRIES: usize = 8192;

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
///
/// # Eviction
///
/// When the cache exceeds `max_entries`, all entries are cleared (simple
/// capacity + clear strategy). Glyphs are cheap to re-rasterize relative
/// to stalling GPU frames, so a full clear on overflow is acceptable.
pub struct GlyphCache {
    cache: HashMap<(FontHandle, u32, u8), CachedGlyph>,
    /// Maximum number of entries before the cache is cleared on overflow.
    max_entries: usize,
}

impl GlyphCache {
    /// Creates a new empty glyph cache with the default capacity limit.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }

    /// Creates a new glyph cache with a custom maximum entry count.
    pub fn with_max_entries(max_entries: usize) -> Self {
        Self {
            cache: HashMap::new(),
            max_entries,
        }
    }

    /// Looks up a cached glyph by font handle, glyph ID, and variant.
    pub fn get(&self, font: FontHandle, glyph_id: u32, variant: u8) -> Option<&CachedGlyph> {
        self.cache.get(&(font, glyph_id, variant))
    }

    /// True when `key` is cached AND its atlas slot is still live.
    ///
    /// Gates on the monotonic `token`, not the `AllocId`: the atlas evicts
    /// off-screen slots under pressure and etagere recycles the numeric
    /// `AllocId` to another glyph, so a stale entry's id can read "present"
    /// while pointing at someone else's pixels. A token mismatch means our slot
    /// was evicted → caller re-materializes (overwriting the stale CPU entry).
    /// We never deallocate the evicted id here: it may already belong to a live
    /// glyph, and freeing it would remove that glyph's slot.
    fn live_hit(&self, key: &(FontHandle, u32, u8), atlas: &Atlas) -> bool {
        self.cache
            .get(key)
            .is_some_and(|cg| atlas.is_live(cg.alloc.alloc_id, cg.alloc.token))
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

        if self.live_hit(&key, atlas) {
            return Ok(false);
        }

        // Evict all entries when at capacity. Deallocate each atlas slot
        // before dropping to prevent slot leaks.
        if self.cache.len() >= self.max_entries {
            log::warn!(
                "GlyphCache at capacity ({} entries); clearing cache to free memory",
                self.max_entries
            );
            for (_, cg) in self.cache.drain() {
                atlas.deallocate(cg.alloc.alloc_id);
            }
        }

        let bitmap = backend.rasterize_glyph(font, glyph_id, variant)?;
        if bitmap.width == 0 || bitmap.height == 0 {
            return Ok(false);
        }

        let alloc = allocate_glyph(atlas, bitmap.width, bitmap.height)
            .map_err(|e| TextError::RasterizationFailed(format!("atlas alloc: {e:?}")))?;

        let padded = pad_with_gutter(&bitmap.alpha, bitmap.width, bitmap.height);
        atlas.upload(queue, alloc.alloc_id, &padded);

        let metrics = GlyphMetrics::from_bitmap(&bitmap);
        self.cache.insert(
            key,
            CachedGlyph {
                alloc,
                metrics,
            },
        );

        Ok(true)
    }

    /// Rasterize+upload variant that resolves the font from the backend's
    /// `font_for(handle)` registry instead of using a caller-supplied font.
    ///
    /// Used by `TextRunBuilder` to dispatch substitute-font glyphs (e.g.,
    /// CJK glyphs the platform shaper rendered with a system fallback face).
    /// Returns `Ok(false)` if the handle isn't registered — caller should
    /// skip the glyph or fall back to the primary font.
    pub fn materialize_by_handle<B: TextBackend>(
        &mut self,
        backend: &B,
        handle: FontHandle,
        glyph_id: u32,
        variant: u8,
        atlas: &mut Atlas,
        queue: &wgpu::Queue,
    ) -> Result<bool, TextError> {
        let Some(font) = backend.font_for(handle) else {
            return Ok(false);
        };
        let key = (handle, glyph_id, variant);

        if self.live_hit(&key, atlas) {
            return Ok(false);
        }

        if self.cache.len() >= self.max_entries {
            log::warn!(
                "GlyphCache at capacity ({} entries); clearing cache to free memory",
                self.max_entries
            );
            for (_, cg) in self.cache.drain() {
                atlas.deallocate(cg.alloc.alloc_id);
            }
        }

        let bitmap = backend.rasterize_glyph(font, glyph_id, variant)?;
        if bitmap.width == 0 || bitmap.height == 0 {
            return Ok(false);
        }

        let alloc = allocate_glyph(atlas, bitmap.width, bitmap.height)
            .map_err(|e| TextError::RasterizationFailed(format!("atlas alloc: {e:?}")))?;

        let padded = pad_with_gutter(&bitmap.alpha, bitmap.width, bitmap.height);
        atlas.upload(queue, alloc.alloc_id, &padded);

        let metrics = GlyphMetrics::from_bitmap(&bitmap);
        self.cache.insert(
            key,
            CachedGlyph {
                alloc,
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

    /// Clears all cached glyphs, returning their atlas slots.
    pub fn clear(&mut self, atlas: &mut Atlas) {
        for (_, cg) in self.cache.drain() {
            atlas.deallocate(cg.alloc.alloc_id);
        }
    }

    /// Clear CPU-side cache state without touching atlas.
    ///
    /// Used after device-lost recovery when the owning Atlas has been dropped
    /// with the old Renderer. All cached `CachedGlyph` entries reference stale
    /// atlas AllocIds and must be discarded.
    pub fn clear_cpu_state(&mut self) {
        self.cache.clear();
    }

    /// Returns the number of cached glyphs.
    ///
    /// Useful for testing and debugging.
    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    /// Returns the configured maximum number of entries before eviction.
    pub fn max_entries(&self) -> usize {
        self.max_entries
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_max_entries_is_set() {
        let cache = GlyphCache::new();
        assert_eq!(cache.max_entries(), DEFAULT_MAX_ENTRIES);
        assert_eq!(cache.cache_len(), 0);
    }

    #[test]
    fn with_max_entries_configures_limit() {
        let cache = GlyphCache::with_max_entries(64);
        assert_eq!(cache.max_entries(), 64);
    }

    #[test]
    fn pad_with_gutter_dimensions() {
        // 2×2 source produces 4×4 padded output with zero border
        let src = vec![1u8, 2, 3, 4];
        let result = pad_with_gutter(&src, 2, 2);
        assert_eq!(result.len(), 4 * 4);
        // top row must be all zeros
        assert_eq!(&result[0..4], &[0u8, 0, 0, 0]);
        // inner row 1: [0, 1, 2, 0]
        assert_eq!(&result[4..8], &[0u8, 1, 2, 0]);
        // inner row 2: [0, 3, 4, 0]
        assert_eq!(&result[8..12], &[0u8, 3, 4, 0]);
        // bottom row must be all zeros
        assert_eq!(&result[12..16], &[0u8, 0, 0, 0]);
    }
}
