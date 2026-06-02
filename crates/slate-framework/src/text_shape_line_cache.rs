//! Content-keyed memoization for single-line text shaping.
//!
//! `TextSystem::shape_line` runs the platform shaper (DirectWrite / CoreText)
//! on every layout pass to size each label. Shaping is deterministic given the
//! font instance and the string, so the result can be memoized. This cache sits
//! on the layout-pass hot path — distinct from the element-keyed
//! `TextShapingCache`, which only covers prepaint / paint — turning a
//! whole-view rebuild from O(visible text) platform-shaper calls into
//! O(visible text) hash-map lookups.
//!
//! Mirrors `slate_text::bounds_cache::RasterBoundsCache`: a flat map with an
//! entry cap and clear-on-overflow eviction. The working set is the on-screen
//! text (hundreds of strings), so a full clear on the rare overflow is the
//! simplest safe strategy.

use std::collections::HashMap;

use slate_text::{FontHandle, ShapedLine, TextError, hash_text};

/// Default entry cap before the cache clears on the next insert.
const DEFAULT_MAX_ENTRIES: usize = 4096;

/// Cache key: font instance + content byte length + 64-bit content hash.
///
/// `FontHandle` encodes face + size + scale, so a DPI or font change yields a
/// fresh key automatically and stale entries fall out via the entry cap. The
/// byte length discriminates strings that collide under the 64-bit hash,
/// mirroring `slate_text::line_layout_cache::LineLayoutCache` (the sibling
/// shaped-line cache).
type ShapeLineKey = (FontHandle, usize, u64);

/// Memoizes `shape_line` output keyed by `(FontHandle, content_hash)`.
///
/// Single-threaded by construction: it lives behind a `RefCell` on the `!Send`
/// `TextSystem`, one per window, so no internal locking is required.
pub(crate) struct ShapeLineCache {
    map: HashMap<ShapeLineKey, ShapedLine>,
    /// Maximum number of entries before the map is cleared on overflow.
    max_entries: usize,
    hits: u64,
    misses: u64,
}

impl ShapeLineCache {
    /// Creates an empty cache with the default entry cap.
    pub(crate) fn new() -> Self {
        Self::with_max_entries(DEFAULT_MAX_ENTRIES)
    }

    /// Creates an empty cache with a custom entry cap (used by tests).
    pub(crate) fn with_max_entries(max_entries: usize) -> Self {
        Self {
            map: HashMap::new(),
            max_entries,
            hits: 0,
            misses: 0,
        }
    }

    /// Returns the cached shaped line for `(handle, content)`, or shapes it via
    /// `shape` on a miss and stores the result.
    ///
    /// On a hit the stored line is cloned and returned without calling `shape`.
    /// On a miss `shape` runs; its `Ok` result is cached and returned, while an
    /// `Err` is propagated and never cached (a transient shaping failure must
    /// not poison the cache). Clears all entries when the cap is reached before
    /// inserting — shaped lines are cheap to recompute.
    pub(crate) fn get_or_shape<F>(
        &mut self,
        handle: FontHandle,
        content: &str,
        shape: F,
    ) -> Result<ShapedLine, TextError>
    where
        F: FnOnce() -> Result<ShapedLine, TextError>,
    {
        let key = (handle, content.len(), hash_text(content));

        if let Some(line) = self.map.get(&key) {
            self.hits += 1;
            return Ok(line.clone());
        }

        self.misses += 1;
        let line = shape()?;
        if self.map.len() >= self.max_entries {
            self.map.clear();
        }
        self.map.insert(key, line.clone());
        Ok(line)
    }

    /// Number of cache hits since construction (testing / profiling).
    pub(crate) fn hits(&self) -> u64 {
        self.hits
    }

    /// Number of cache misses since construction (testing / profiling).
    pub(crate) fn misses(&self) -> u64 {
        self.misses
    }

    /// Number of entries currently held.
    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slate_text::Direction;

    /// Build a distinguishable `ShapedLine` so tests can tell a cached value
    /// from a freshly shaped one by its width.
    fn line(width_lpx: f32) -> ShapedLine {
        ShapedLine {
            glyphs: Vec::new(),
            width_lpx,
            ascent_lpx: 0.0,
            descent_lpx: 0.0,
            y_offset_lpx: 0.0,
            base_direction: Direction::Ltr,
            runs: Vec::new(),
        }
    }

    #[test]
    fn second_lookup_is_a_hit() {
        let mut cache = ShapeLineCache::new();
        let handle = FontHandle::from_face_id(0x1, 16.0, 1.0);
        let mut calls = 0;

        let first = cache
            .get_or_shape(handle, "Label", || {
                calls += 1;
                Ok(line(10.0))
            })
            .unwrap();
        assert_eq!(calls, 1);
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.hits(), 0);

        // The closure returns a different width — if it ran, we'd see 99.0.
        let second = cache
            .get_or_shape(handle, "Label", || {
                calls += 1;
                Ok(line(99.0))
            })
            .unwrap();
        assert_eq!(calls, 1, "second call must hit the cache, not reshape");
        assert_eq!(cache.hits(), 1);
        assert_eq!(
            second.width_lpx, first.width_lpx,
            "hit must return the cached line, not the closure's value"
        );
    }

    #[test]
    fn distinct_font_or_text_miss() {
        let mut cache = ShapeLineCache::new();
        let h16 = FontHandle::from_face_id(0x1, 16.0, 1.0);
        let h18 = FontHandle::from_face_id(0x1, 18.0, 1.0); // size differs → new handle

        cache.get_or_shape(h16, "A", || Ok(line(1.0))).unwrap();
        cache.get_or_shape(h18, "A", || Ok(line(2.0))).unwrap(); // different handle
        cache.get_or_shape(h16, "B", || Ok(line(3.0))).unwrap(); // different content

        assert_eq!(cache.misses(), 3);
        assert_eq!(cache.hits(), 0);
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn eviction_clears_on_overflow() {
        let mut cache = ShapeLineCache::with_max_entries(2);
        let handle = FontHandle::from_face_id(0x1, 16.0, 1.0);

        cache.get_or_shape(handle, "one", || Ok(line(1.0))).unwrap();
        cache.get_or_shape(handle, "two", || Ok(line(2.0))).unwrap();
        assert_eq!(cache.len(), 2);

        // Third distinct entry at the cap → clear, then insert only the new one.
        cache
            .get_or_shape(handle, "three", || Ok(line(3.0)))
            .unwrap();
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn shape_error_is_not_cached() {
        let mut cache = ShapeLineCache::new();
        let handle = FontHandle::from_face_id(0x1, 16.0, 1.0);

        let result = cache.get_or_shape(handle, "x", || {
            Err(TextError::ShapingFailed("transient".into()))
        });

        assert!(result.is_err());
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.len(), 0, "errors must not be cached");
    }
}
