//! Raster bounds cache for glyph pre-rasterization queries.
//!
//! Caches `GlyphBounds` keyed by `(FontHandle, glyph_id)` — not variant/dilation,
//! since bounds are invariant across sub-pixel offsets.

use fxhash::FxHashMap;
use parking_lot::RwLock;

use crate::font_handle::FontHandle;
use crate::types::GlyphBounds;

/// Default maximum number of entries before the bounds cache is cleared.
const DEFAULT_MAX_ENTRIES: usize = 16384;

/// Cache key: (font handle, glyph ID).
type BoundsCacheKey = (FontHandle, u32);

/// Thread-safe cache for glyph raster bounds.
///
/// Uses `parking_lot::RwLock` for efficient concurrent reads.
/// Bounds are invariant across sub-pixel variants and dilation levels,
/// so the cache key excludes those parameters.
///
/// # Eviction
///
/// When the cache exceeds `max_entries`, all entries are cleared on the
/// next insert. Bounds are cheap to recompute, so a full clear on overflow
/// is the simplest safe strategy.
pub struct RasterBoundsCache {
    inner: RwLock<FxHashMap<BoundsCacheKey, GlyphBounds>>,
    /// Maximum number of entries before the cache is cleared on overflow.
    max_entries: usize,
}

impl RasterBoundsCache {
    /// Creates a new empty bounds cache with the default capacity limit.
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(FxHashMap::default()),
            max_entries: DEFAULT_MAX_ENTRIES,
        }
    }

    /// Creates a new bounds cache with a custom maximum entry count.
    pub fn with_max_entries(max_entries: usize) -> Self {
        Self {
            inner: RwLock::new(FxHashMap::default()),
            max_entries,
        }
    }

    /// Returns the configured maximum number of entries before eviction.
    pub fn max_entries(&self) -> usize {
        self.max_entries
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
    /// Clears all entries when capacity is exceeded before inserting.
    pub fn insert(
        &self,
        font: FontHandle,
        glyph_id: u32,
        bounds: GlyphBounds,
    ) -> Option<GlyphBounds> {
        let mut guard = self.inner.write();
        if guard.len() >= self.max_entries {
            guard.clear();
        }
        guard.insert((font, glyph_id), bounds)
    }

    /// Gets or inserts bounds using a closure.
    ///
    /// If the key exists, returns the cached value without calling `f`.
    /// Otherwise, calls `f()` to compute the value, inserts it, and returns it.
    ///
    /// Thread-safe: re-checks under write lock to avoid TOCTOU race.
    /// Clears all entries when capacity is exceeded before inserting.
    pub fn get_or_insert_with<F>(&self, font: FontHandle, glyph_id: u32, f: F) -> GlyphBounds
    where
        F: FnOnce() -> GlyphBounds,
    {
        let key = (font, glyph_id);

        if let Some(bounds) = self.inner.read().get(&key).copied() {
            return bounds;
        }

        let mut guard = self.inner.write();
        // Re-check under write lock (TOCTOU guard).
        if let Some(bounds) = guard.get(&key).copied() {
            return bounds;
        }
        // Evict all entries when at capacity — bounds are cheap to recompute.
        if guard.len() >= self.max_entries {
            guard.clear();
        }
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
    fn eviction_clears_on_overflow_insert() {
        let cache = RasterBoundsCache::with_max_entries(4);
        let font = FontHandle::from_ptr_size_scale(0x3000 as *const u8, 12.0, 1.0);
        let bounds = GlyphBounds {
            width: 8,
            height: 8,
        };

        // Fill to capacity
        for glyph_id in 0..4u32 {
            cache.insert(font, glyph_id, bounds);
        }
        assert_eq!(cache.len(), 4);

        // Inserting one more should trigger a clear, then insert the new entry
        cache.insert(font, 99, bounds);
        // After clear + insert, len should be 1 (only the new entry)
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn eviction_clears_on_overflow_get_or_insert() {
        let cache = RasterBoundsCache::with_max_entries(3);
        let font = FontHandle::from_ptr_size_scale(0x4000 as *const u8, 10.0, 1.0);
        let bounds = GlyphBounds {
            width: 5,
            height: 5,
        };

        for glyph_id in 0..3u32 {
            cache.insert(font, glyph_id, bounds);
        }
        assert_eq!(cache.len(), 3);

        // get_or_insert_with for a new key at capacity should evict then insert
        let result = cache.get_or_insert_with(font, 200, || GlyphBounds {
            width: 7,
            height: 7,
        });
        assert_eq!(
            result,
            GlyphBounds {
                width: 7,
                height: 7
            }
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn whitespace_detection() {
        assert!(GlyphBounds::ZERO.is_whitespace());
        assert!(GlyphBounds {
            width: 0,
            height: 10
        }
        .is_whitespace());
        assert!(GlyphBounds {
            width: 10,
            height: 0
        }
        .is_whitespace());
        assert!(!GlyphBounds {
            width: 10,
            height: 10
        }
        .is_whitespace());
    }
}
