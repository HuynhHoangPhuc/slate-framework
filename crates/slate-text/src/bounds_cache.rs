//! Raster bounds cache for glyph pre-rasterization queries.
//!
//! Caches `GlyphBounds` keyed by `(FontHandle, glyph_id)` — not variant/dilation,
//! since bounds are invariant across sub-pixel offsets.

use fxhash::FxHashMap;
use parking_lot::RwLock;

use crate::font_handle::FontHandle;
use crate::types::GlyphBounds;

/// Cache key: (font handle, glyph ID).
type BoundsCacheKey = (FontHandle, u32);

/// Thread-safe cache for glyph raster bounds.
///
/// Uses `parking_lot::RwLock` for efficient concurrent reads.
/// Bounds are invariant across sub-pixel variants and dilation levels,
/// so the cache key excludes those parameters.
pub struct RasterBoundsCache {
    inner: RwLock<FxHashMap<BoundsCacheKey, GlyphBounds>>,
}

impl RasterBoundsCache {
    /// Creates a new empty bounds cache.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(FxHashMap::default()),
        }
    }

    /// Looks up cached bounds for a glyph.
    ///
    /// Returns `None` if not cached; caller should compute and insert.
    #[inline]
    pub fn get(&self, font: FontHandle, glyph_id: u32) -> Option<GlyphBounds> {
        self.inner.read().get(&(font, glyph_id)).copied()
    }

    /// Inserts bounds into the cache.
    ///
    /// Returns the previous value if the key was present.
    pub fn insert(
        &self,
        font: FontHandle,
        glyph_id: u32,
        bounds: GlyphBounds,
    ) -> Option<GlyphBounds> {
        self.inner.write().insert((font, glyph_id), bounds)
    }

    /// Gets or inserts bounds using a closure.
    ///
    /// If the key exists, returns the cached value without calling `f`.
    /// Otherwise, calls `f()` to compute the value, inserts it, and returns it.
    ///
    /// Thread-safe: re-checks under write lock to avoid TOCTOU race.
    pub fn get_or_insert_with<F>(&self, font: FontHandle, glyph_id: u32, f: F) -> GlyphBounds
    where
        F: FnOnce() -> GlyphBounds,
    {
        let key = (font, glyph_id);

        if let Some(bounds) = self.inner.read().get(&key).copied() {
            return bounds;
        }

        let mut guard = self.inner.write();
        *guard.entry(key).or_insert_with(f)
    }

    /// Returns the number of cached entries.
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// Returns true if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.inner.read().is_empty()
    }

    /// Clears all cached entries.
    pub fn clear(&self) {
        self.inner.write().clear();
    }
}

impl Default for RasterBoundsCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_insert_and_get() {
        let cache = RasterBoundsCache::new();
        let font = FontHandle::from_ptr_size_scale(0x1000 as *const u8, 16.0, 2.0);

        assert!(cache.get(font, 65).is_none());

        let bounds = GlyphBounds {
            width: 10,
            height: 20,
        };
        cache.insert(font, 65, bounds);

        assert_eq!(cache.get(font, 65), Some(bounds));
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn cache_get_or_insert() {
        let cache = RasterBoundsCache::new();
        let font = FontHandle::from_ptr_size_scale(0x2000 as *const u8, 16.0, 1.0);

        let mut call_count = 0;
        let bounds = cache.get_or_insert_with(font, 66, || {
            call_count += 1;
            GlyphBounds {
                width: 5,
                height: 10,
            }
        });

        assert_eq!(
            bounds,
            GlyphBounds {
                width: 5,
                height: 10
            }
        );
        assert_eq!(call_count, 1);

        let bounds2 = cache.get_or_insert_with(font, 66, || {
            call_count += 1;
            GlyphBounds {
                width: 99,
                height: 99,
            }
        });

        assert_eq!(
            bounds2,
            GlyphBounds {
                width: 5,
                height: 10
            }
        );
        assert_eq!(call_count, 1);
    }

    #[test]
    fn whitespace_detection() {
        assert!(GlyphBounds::ZERO.is_whitespace());
        assert!(
            GlyphBounds {
                width: 0,
                height: 10
            }
            .is_whitespace()
        );
        assert!(
            GlyphBounds {
                width: 10,
                height: 0
            }
            .is_whitespace()
        );
        assert!(
            !GlyphBounds {
                width: 10,
                height: 10
            }
            .is_whitespace()
        );
    }
}
