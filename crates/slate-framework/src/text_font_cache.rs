//! Identity-keyed memoization for loaded platform fonts.
//!
//! `TextSystem::load_font` / `load_font_from_bytes` build a platform font from
//! scratch on every call — on Windows that means a full TTF parse plus a stack
//! of DirectWrite COM constructions (font file, font face, font set, font
//! collection, text format). Because the reactive view is rebuilt every frame,
//! each `Text` element starts with no font and reloads it, so a screen with N
//! text nodes pays N full font loads per frame — the dominant per-interaction
//! cost on dense screens.
//!
//! A font is fully determined by its source (bundled byte slice or system
//! family name) plus size and scale, and is immutable after load (shaping,
//! measuring, and rasterizing only read it). So the loaded font can be memoized
//! and shared: the platform font types `Clone` by bumping a COM / CoreFoundation
//! refcount, never re-parsing. This turns N loads/frame into N cheap clones.
//!
//! Mirrors [`crate::text_shape_line_cache::ShapeLineCache`]: a flat map with an
//! entry cap and clear-on-overflow eviction. The working set is the handful of
//! `(font, size, scale)` combinations on screen, so a clear on the rare overflow
//! (e.g. churn from repeated DPI changes) is the simplest safe strategy.

use std::collections::HashMap;

/// Default entry cap before the cache clears on the next insert.
const DEFAULT_MAX_ENTRIES: usize = 256;

/// Identity of a loaded font: its source plus the size/scale it was built at.
///
/// Floats are keyed by their exact bit pattern (`f32::to_bits`) so a size or
/// scale change yields a distinct entry. Byte-slice sources are keyed by the
/// `&'static` pointer + length (stable identity for bundled fonts); system
/// fonts by family name.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum FontCacheKey {
    /// Font loaded from a static byte slice (bundled font).
    Bytes {
        ptr: usize,
        len: usize,
        size_bits: u32,
        scale_bits: u32,
    },
    /// Font loaded from the system collection by family name.
    Family {
        family: String,
        size_bits: u32,
        scale_bits: u32,
    },
}

impl FontCacheKey {
    /// Key for a bundled-byte font load.
    pub(crate) fn from_bytes(bytes: &'static [u8], size_lpx: f32, scale: f32) -> Self {
        FontCacheKey::Bytes {
            ptr: bytes.as_ptr() as usize,
            len: bytes.len(),
            size_bits: size_lpx.to_bits(),
            scale_bits: scale.to_bits(),
        }
    }

    /// Key for a system-family font load.
    pub(crate) fn from_family(family: &str, size_lpx: f32, scale: f32) -> Self {
        FontCacheKey::Family {
            family: family.to_owned(),
            size_bits: size_lpx.to_bits(),
            scale_bits: scale.to_bits(),
        }
    }
}

/// Memoizes loaded fonts keyed by [`FontCacheKey`].
///
/// Generic over the stored value so the get/insert/evict/counter logic can be
/// unit-tested with a trivial `Clone` stand-in; `TextSystem` instantiates it as
/// `FontCache<PlatformFont>`. Single-threaded by construction: it lives on the
/// `!Send` `TextSystem`, one per window, so no locking is required.
pub(crate) struct FontCache<V> {
    map: HashMap<FontCacheKey, V>,
    /// Maximum number of entries before the map is cleared on overflow.
    max_entries: usize,
    hits: u64,
    misses: u64,
}

impl<V: Clone> FontCache<V> {
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

    /// Returns a clone of the cached value for `key`, or loads it via `load` on a
    /// miss and stores the result.
    ///
    /// On a hit the stored value is cloned (a font clone is a refcount bump) and
    /// returned without calling `load`. On a miss `load` runs; its `Ok` result is
    /// cached and returned, while an `Err` is propagated and never cached. Clears
    /// all entries when the cap is reached before inserting — a font is cheap to
    /// reload on the next miss.
    pub(crate) fn get_or_load<F, E>(&mut self, key: FontCacheKey, load: F) -> Result<V, E>
    where
        F: FnOnce() -> Result<V, E>,
    {
        if let Some(value) = self.map.get(&key) {
            self.hits += 1;
            return Ok(value.clone());
        }

        self.misses += 1;
        let value = load()?;
        if self.map.len() >= self.max_entries {
            self.map.clear();
        }
        self.map.insert(key, value.clone());
        Ok(value)
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

    static FONT_A: &[u8] = b"font-a-bytes";
    static FONT_B: &[u8] = b"font-b-bytes-longer";

    #[test]
    fn bytes_key_identity_depends_on_source_size_scale() {
        let base = FontCacheKey::from_bytes(FONT_A, 16.0, 1.0);
        // Same source + size + scale → equal key (cache hit).
        assert_eq!(base, FontCacheKey::from_bytes(FONT_A, 16.0, 1.0));
        // Size, scale, or source change → distinct key (cache miss).
        assert_ne!(base, FontCacheKey::from_bytes(FONT_A, 18.0, 1.0));
        assert_ne!(base, FontCacheKey::from_bytes(FONT_A, 16.0, 2.0));
        assert_ne!(base, FontCacheKey::from_bytes(FONT_B, 16.0, 1.0));
    }

    #[test]
    fn family_key_identity_depends_on_name_size_scale() {
        let base = FontCacheKey::from_family("Segoe UI", 14.0, 1.0);
        assert_eq!(base, FontCacheKey::from_family("Segoe UI", 14.0, 1.0));
        assert_ne!(base, FontCacheKey::from_family("Arial", 14.0, 1.0));
        assert_ne!(base, FontCacheKey::from_family("Segoe UI", 15.0, 1.0));
        assert_ne!(base, FontCacheKey::from_family("Segoe UI", 14.0, 1.5));
    }

    #[test]
    fn bytes_and_family_keys_never_collide() {
        assert_ne!(
            FontCacheKey::from_bytes(FONT_A, 16.0, 1.0),
            FontCacheKey::from_family("font-a-bytes", 16.0, 1.0),
        );
    }

    /// A cache miss runs `load` once and stores the result; the next request for
    /// the same key is a hit that never re-runs `load`.
    #[test]
    fn miss_then_hit_runs_loader_once() {
        let mut cache: FontCache<u32> = FontCache::new();
        let key = FontCacheKey::from_bytes(FONT_A, 16.0, 1.0);
        let mut loads = 0u32;

        let first = cache
            .get_or_load(key.clone(), || {
                loads += 1;
                Ok::<_, ()>(10)
            })
            .unwrap();
        assert_eq!(first, 10);
        assert_eq!(cache.misses(), 1);

        // Loader returns a different value — if it ran, we'd see 99.
        let second = cache
            .get_or_load(key, || {
                loads += 1;
                Ok::<_, ()>(99)
            })
            .unwrap();
        assert_eq!(second, 10, "hit must return the cached value");
        assert_eq!(loads, 1, "second request must hit, not reload");
        assert_eq!(cache.hits(), 1);
    }

    #[test]
    fn error_is_not_cached() {
        let mut cache: FontCache<u32> = FontCache::new();
        let key = FontCacheKey::from_bytes(FONT_A, 16.0, 1.0);
        let result = cache.get_or_load(key, || Err::<u32, &str>("transient"));
        assert!(result.is_err());
        assert_eq!(cache.misses(), 1);
        assert_eq!(cache.len(), 0, "errors must not be cached");
    }

    #[test]
    fn eviction_clears_on_overflow() {
        let mut cache: FontCache<u32> = FontCache::with_max_entries(2);
        cache
            .get_or_load(FontCacheKey::from_bytes(FONT_A, 1.0, 1.0), || Ok::<_, ()>(1))
            .unwrap();
        cache
            .get_or_load(FontCacheKey::from_bytes(FONT_A, 2.0, 1.0), || Ok::<_, ()>(2))
            .unwrap();
        assert_eq!(cache.len(), 2);
        // Third distinct entry at the cap → clear, then insert only the new one.
        cache
            .get_or_load(FontCacheKey::from_bytes(FONT_A, 3.0, 1.0), || Ok::<_, ()>(3))
            .unwrap();
        assert_eq!(cache.len(), 1);
    }
}
