//! Text shaping cache for paint optimization.
//!
//! Phase 6 (D10): Cache pre-atlas `Vec<ShapedGlyph>` per Text element to skip
//! cosmic-text shaping (~70% of paint cost) when content/style/bounds unchanged.
//!
//! # Architecture
//!
//! - **TextShapingCache**: per-frame cache keyed by `(ElementId, input_hash)`
//! - **CachedTextShape**: stores `Vec<ShapedGlyph>` (pre-atlas) + metadata
//! - Atlas resolution happens at replay — eviction-safe (F5)
//! - Leaf-only caching (F4 Strategy L): only Text is cached; Div returns 0 → never cached
//!
//! # Post-v1 Expansion
//!
//! This file is named `paint_cache.rs` (not `text_shaping_cache.rs`) so the post-v1
//! expansion to a generic PaintCache (DrawCmd enum, Div caching) is rename-free.

// Allow dead_code for API methods designed for future use (post-v1 expansion)
#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::mem::size_of;
use std::rc::Weak;

use slate_renderer::RendererObserver;
use slate_text::types::{FontId, ShapedGlyph, ShapedLine};

use crate::types::{Bounds, ElementId};

/// Default memory cap: 50 MB
const DEFAULT_MEMORY_CAP: usize = 50 * 1024 * 1024;

/// Per-element size cap: 64 KB
const PER_ELEMENT_SIZE_CAP: usize = 64 * 1024;

/// Cache for pre-atlas shaped text to skip shaping on unchanged Text elements.
///
/// Keyed by `(ElementId, input_hash)`. On cache hit, returns the stored
/// `Vec<ShapedLine>` (pre-atlas). Caller re-resolves atlas slots at paint time
/// (F5: atlas-eviction-safe).
///
/// # GC Semantics
///
/// Entries not accessed for 2+ consecutive frames are dropped by `gc()`.
/// Matches `StateRegistry` GC semantics for consistency.
pub struct TextShapingCache {
    entries: HashMap<ElementId, CachedTextShape>,
    current_frame: u64,
    memory_used: usize,
    memory_cap: usize,
    // Metrics for debugging/benchmarking
    hit_count: u64,
    miss_count: u64,
}

/// Cached shaped text for a single Text element.
pub struct CachedTextShape {
    /// Hash of (content, font, size, color, bounds) — used to detect staleness.
    pub input_hash: u64,
    /// Pre-atlas shaped lines (one or more for wrapped text).
    pub lines: Vec<ShapedLine>,
    /// Font ID used for shaping (for atlas resolution).
    pub font_id: FontId,
    /// Font size in logical pixels (for atlas resolution).
    pub size_px: f32,
    /// Frame counter when last accessed.
    pub last_seen_frame: u64,
    /// Approximate size in bytes (for memory cap enforcement).
    pub size_bytes: usize,
}

impl CachedTextShape {
    fn empty(frame: u64) -> Self {
        Self {
            input_hash: 0,
            lines: Vec::new(),
            font_id: FontId::default(),
            size_px: 0.0,
            last_seen_frame: frame,
            size_bytes: 0,
        }
    }

    #[allow(clippy::manual_slice_size_calculation)] // Intentional: we want content size, not slice header
    fn compute_size(lines: &[ShapedLine]) -> usize {
        let glyph_count: usize = lines.iter().map(|l| l.glyphs.len()).sum();
        glyph_count * size_of::<ShapedGlyph>() + lines.len() * size_of::<ShapedLine>()
    }
}

impl TextShapingCache {
    /// Create a new cache with default 50 MB memory cap.
    pub fn new() -> Self {
        Self::with_memory_cap(DEFAULT_MEMORY_CAP)
    }

    /// Create a new cache with custom memory cap.
    pub fn with_memory_cap(memory_cap: usize) -> Self {
        Self {
            entries: HashMap::new(),
            current_frame: 0,
            memory_used: 0,
            memory_cap,
            hit_count: 0,
            miss_count: 0,
        }
    }

    /// Check if cache has an entry for the given element with matching hash.
    ///
    /// Returns the cached lines if hit, None if miss. Does NOT update the cache
    /// on miss — use `with_shape` for the full cache-or-compute pattern.
    pub fn get(&mut self, id: ElementId, input_hash: u64) -> Option<&[ShapedLine]> {
        if input_hash == 0 {
            return None; // Sentinel: caching disabled for this element
        }

        if let Some(entry) = self.entries.get_mut(&id)
            && entry.input_hash == input_hash
        {
            entry.last_seen_frame = self.current_frame;
            self.hit_count += 1;
            return Some(&entry.lines);
        }
        None
    }

    /// Insert or update a cache entry.
    ///
    /// Skips insertion if the shaped lines exceed the per-element size cap (64 KB).
    pub fn insert(
        &mut self,
        id: ElementId,
        input_hash: u64,
        lines: Vec<ShapedLine>,
        font_id: FontId,
        size_px: f32,
    ) {
        if input_hash == 0 {
            return; // Sentinel: caching disabled
        }

        let new_size = CachedTextShape::compute_size(&lines);

        // Skip caching if exceeds per-element cap
        if new_size > PER_ELEMENT_SIZE_CAP {
            self.miss_count += 1;
            return;
        }

        // Update memory accounting
        if let Some(old_entry) = self.entries.get(&id) {
            self.memory_used = self.memory_used.saturating_sub(old_entry.size_bytes);
        }
        self.memory_used += new_size;

        let entry = CachedTextShape {
            input_hash,
            lines,
            font_id,
            size_px,
            last_seen_frame: self.current_frame,
            size_bytes: new_size,
        };

        self.entries.insert(id, entry);
        self.miss_count += 1;
    }

    /// Callback API: cache lookup with fallback shaping.
    ///
    /// On cache hit, calls `on_hit` with the cached lines. On miss, calls
    /// `shape_fresh` to produce fresh lines, caches them, then calls `on_hit`.
    ///
    /// `hash == 0` is the sentinel for "caching disabled" — calls `shape_fresh`
    /// then `on_hit` without caching.
    ///
    /// # Why Callback API?
    ///
    /// Avoids returning a borrowed reference while the caller also needs
    /// `&mut TextSystem` / `&mut Atlas` for atlas resolution.
    pub fn with_shape<S, R>(
        &mut self,
        id: ElementId,
        input_hash: u64,
        font_id: FontId,
        size_px: f32,
        shape_fresh: S,
        on_hit: impl FnOnce(&[ShapedLine]) -> R,
    ) -> R
    where
        S: FnOnce() -> Vec<ShapedLine>,
    {
        // Sentinel: caching disabled
        if input_hash == 0 {
            let lines = shape_fresh();
            return on_hit(&lines);
        }

        // Check for cache hit
        if let Some(entry) = self.entries.get_mut(&id)
            && entry.input_hash == input_hash
        {
            entry.last_seen_frame = self.current_frame;
            self.hit_count += 1;
            return on_hit(&entry.lines);
        }

        // Cache miss: shape fresh and insert
        let lines = shape_fresh();
        let new_size = CachedTextShape::compute_size(&lines);

        // Only cache if under per-element cap
        let result = on_hit(&lines);

        if new_size <= PER_ELEMENT_SIZE_CAP {
            // Update memory accounting
            if let Some(old_entry) = self.entries.get(&id) {
                self.memory_used = self.memory_used.saturating_sub(old_entry.size_bytes);
            }
            self.memory_used += new_size;

            let entry = CachedTextShape {
                input_hash,
                lines,
                font_id,
                size_px,
                last_seen_frame: self.current_frame,
                size_bytes: new_size,
            };
            self.entries.insert(id, entry);
        }

        self.miss_count += 1;
        result
    }

    /// Advance the frame counter. Call once per frame, post-paint.
    pub fn advance_frame(&mut self) {
        self.current_frame = self.current_frame.wrapping_add(1);
    }

    /// Garbage collect entries not seen for 2+ frames.
    ///
    /// Also triggers LRU eviction if memory cap exceeded.
    pub fn gc(&mut self) {
        let cutoff = self.current_frame.saturating_sub(2);
        self.entries.retain(|_, entry| {
            if entry.last_seen_frame >= cutoff {
                true
            } else {
                self.memory_used = self.memory_used.saturating_sub(entry.size_bytes);
                false
            }
        });

        // LRU eviction if over memory cap
        if self.memory_used > self.memory_cap {
            self.lru_evict_quarter();
        }
    }

    /// Evict oldest 25% of entries by last_seen_frame.
    fn lru_evict_quarter(&mut self) {
        if self.entries.is_empty() {
            return;
        }

        // Collect entries sorted by last_seen_frame (oldest first)
        let mut ids_by_age: Vec<_> = self
            .entries
            .iter()
            .map(|(id, e)| (*id, e.last_seen_frame, e.size_bytes))
            .collect();
        ids_by_age.sort_by_key(|(_, frame, _)| *frame);

        // Evict oldest 25%
        let evict_count = ids_by_age.len().div_ceil(4);
        for (id, _, size_bytes) in ids_by_age.into_iter().take(evict_count) {
            self.entries.remove(&id);
            self.memory_used = self.memory_used.saturating_sub(size_bytes);
        }
    }

    /// Drop all cached shaped lines.
    ///
    /// Used after device-lost recovery when atlas slots referenced by stored
    /// `Vec<ShapedGlyph>` AllocIds are invalid. Preserves frame counter and
    /// metrics for telemetry continuity.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.memory_used = 0;
    }

    /// Get current frame counter.
    pub fn current_frame(&self) -> u64 {
        self.current_frame
    }

    /// Get total memory used by cached entries.
    pub fn memory_used(&self) -> usize {
        self.memory_used
    }

    /// Get cache hit count (for benchmarking).
    pub fn hit_count(&self) -> u64 {
        self.hit_count
    }

    /// Get cache miss count (for benchmarking).
    pub fn miss_count(&self) -> u64 {
        self.miss_count
    }

    /// Get number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for TextShapingCache {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Hash Helpers (F2: f32 quantization for Bounds/Color)
// =============================================================================

/// Quantize f32 to 1/256-px and hash.
///
/// Provides sub-pixel precision at common DPRs. Collisions on positions less
/// than 1/256 px apart are intentional cache hits.
///
/// NaN-safe: NaN hashes as 0.
#[inline]
pub fn hash_f32<H: Hasher>(v: f32, h: &mut H) {
    let q = if v.is_nan() {
        0
    } else {
        (v * 256.0).round().clamp(i32::MIN as f32, i32::MAX as f32) as i32
    };
    q.hash(h);
}

/// Hash Bounds (origin + size, 4 f32s).
#[inline]
pub fn hash_bounds<H: Hasher>(bounds: &Bounds, h: &mut H) {
    hash_f32(bounds.origin.x, h);
    hash_f32(bounds.origin.y, h);
    hash_f32(bounds.size.width, h);
    hash_f32(bounds.size.height, h);
}

/// Hash RGBA color (4 f32s).
#[inline]
pub fn hash_color<H: Hasher>(color: &[f32; 4], h: &mut H) {
    for &c in color {
        hash_f32(c, h);
    }
}

// ---------------------------------------------------------------------------
// Phase 4: RendererObserver implementation for cache invalidation
// ---------------------------------------------------------------------------

/// Observer that clears TextShapingCache on device recreation.
///
/// Registered with the Renderer; fires when device is successfully rebuilt.
/// Clears stale shaped text entries that may reference invalid state.
pub struct TextShapingCacheObserver {
    inner: Weak<RefCell<TextShapingCache>>,
}

impl TextShapingCacheObserver {
    /// Create a new observer wrapping a weak ref to the text shaping cache.
    pub fn new(inner: Weak<RefCell<TextShapingCache>>) -> Self {
        Self { inner }
    }
}

impl RendererObserver for TextShapingCacheObserver {
    fn on_renderer_recreated(&self, generation: u64) {
        log::debug!(target: "slate::device_lost",
            "TextShapingCacheObserver: clearing cache (gen={})", generation);
        if let Some(strong) = self.inner.upgrade() {
            strong.borrow_mut().clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slate_text::types::ShapedLine;

    fn make_shaped_line(glyph_count: usize) -> ShapedLine {
        ShapedLine {
            glyphs: (0..glyph_count)
                .map(|i| ShapedGlyph {
                    glyph_id: i as u32,
                    font_id: FontId::PRIMARY,
                    font_handle: Default::default(),
                    x_advance_lpx: 10.0,
                    position_lpx: [0.0, 0.0],
                    cluster: 0,
                })
                .collect(),
            width_lpx: glyph_count as f32 * 10.0,
            ascent_lpx: 14.0,
            descent_lpx: -4.0,
            y_offset_lpx: 0.0,
        }
    }

    #[test]
    fn cache_hit_miss() {
        let mut cache = TextShapingCache::new();
        let id = ElementId::from_raw(1);
        let hash = 12345;

        // First access: miss
        assert!(cache.get(id, hash).is_none());

        // Insert
        cache.insert(id, hash, vec![make_shaped_line(5)], FontId::PRIMARY, 14.0);

        // Second access: hit
        let lines = cache.get(id, hash);
        assert!(lines.is_some());
        assert_eq!(lines.unwrap().len(), 1);
        assert_eq!(lines.unwrap()[0].glyphs.len(), 5);

        // Different hash: miss
        assert!(cache.get(id, 99999).is_none());
    }

    #[test]
    fn cache_gc() {
        let mut cache = TextShapingCache::new();
        let id = ElementId::from_raw(1);
        let hash = 12345;

        // Frame 0: insert
        cache.insert(id, hash, vec![make_shaped_line(5)], FontId::PRIMARY, 14.0);
        assert_eq!(cache.len(), 1);

        // Frame 1: advance, gc — entry survives (1 frame gap)
        cache.advance_frame();
        cache.gc();
        assert_eq!(cache.len(), 1);

        // Frame 2: advance, gc — entry survives (2 frame gap, cutoff is current - 2)
        cache.advance_frame();
        cache.gc();
        assert_eq!(cache.len(), 1);

        // Frame 3: advance, gc — entry evicted (3 frame gap, last_seen=0, cutoff=1)
        cache.advance_frame();
        cache.gc();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn sentinel_hash_zero_bypasses_cache() {
        let mut cache = TextShapingCache::new();
        let id = ElementId::from_raw(1);

        // Insert with hash 0 — should not cache
        cache.insert(id, 0, vec![make_shaped_line(5)], FontId::PRIMARY, 14.0);
        assert!(cache.get(id, 0).is_none());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn with_shape_callback() {
        let mut cache = TextShapingCache::new();
        let id = ElementId::from_raw(1);
        let hash = 12345;

        let mut shape_called = false;

        // First call: shape_fresh is called
        let result = cache.with_shape(
            id,
            hash,
            FontId::PRIMARY,
            14.0,
            || {
                shape_called = true;
                vec![make_shaped_line(5)]
            },
            |lines| lines.len(),
        );
        assert!(shape_called);
        assert_eq!(result, 1);

        // Second call: shape_fresh NOT called (cache hit)
        shape_called = false;
        let result = cache.with_shape(
            id,
            hash,
            FontId::PRIMARY,
            14.0,
            || {
                shape_called = true;
                vec![make_shaped_line(5)]
            },
            |lines| lines.len(),
        );
        assert!(!shape_called);
        assert_eq!(result, 1);
    }

    #[test]
    fn hash_f32_nan_safe() {
        use std::collections::hash_map::DefaultHasher;

        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();

        hash_f32(f32::NAN, &mut h1);
        hash_f32(f32::NAN, &mut h2);

        // NaN should hash consistently
        assert_eq!(h1.finish(), h2.finish());
    }

    #[test]
    fn hash_f32_quantization() {
        use std::collections::hash_map::DefaultHasher;

        // Values < 1/256 apart should hash the same
        let mut h1 = DefaultHasher::new();
        let mut h2 = DefaultHasher::new();

        hash_f32(100.0, &mut h1);
        hash_f32(100.001, &mut h2); // ~0.001 < 1/256 ≈ 0.0039

        assert_eq!(h1.finish(), h2.finish());

        // Values > 1/256 apart should hash differently
        let mut h3 = DefaultHasher::new();
        let mut h4 = DefaultHasher::new();

        hash_f32(100.0, &mut h3);
        hash_f32(100.01, &mut h4); // 0.01 > 1/256 ≈ 0.0039

        assert_ne!(h3.finish(), h4.finish());
    }

    #[test]
    fn memory_cap_lru_eviction() {
        // Create cache with small cap
        let mut cache = TextShapingCache::with_memory_cap(1000);
        let font_id = FontId::PRIMARY;

        // Insert multiple entries that exceed cap
        for i in 0..10 {
            let id = ElementId::from_raw(i);
            // Each line has ~100 bytes (10 glyphs * ~10 bytes each)
            cache.insert(id, i + 1, vec![make_shaped_line(10)], font_id, 14.0);
            cache.advance_frame(); // Stagger frames so LRU has ordering
        }

        // Trigger GC
        cache.gc();

        // Should have evicted oldest entries
        assert!(cache.len() < 10);
        assert!(cache.memory_used() <= 1000 || cache.len() <= 7); // ~25% evicted
    }
}
