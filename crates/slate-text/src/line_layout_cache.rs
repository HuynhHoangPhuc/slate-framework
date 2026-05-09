//! Line-layout cache with two-frame rolling pattern.
//!
//! Caches `ShapedLine` results by `(text_hash, byte_len, FontHandle)` to avoid
//! redundant shaping of static text. Uses GPUI-style two-frame rolling: current
//! frame is writable, previous frame is read-only and entries migrate on hit.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use parking_lot::Mutex;
use rustc_hash::{FxHashMap, FxHasher};

use crate::{FontHandle, ShapedLine};

/// Cache key: (text_hash, byte_len, font_handle).
/// `byte_len` reduces hash collision probability for different strings.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
struct CacheKey {
    text_hash: u64,
    byte_len: usize,
    font: FontHandle,
}

/// Two-frame line-layout cache.
///
/// # Usage
///
/// 1. Use `get_or_shape()` to retrieve cached lines or shape new ones
/// 2. Call `finish_frame()` at the end of each frame
///
/// Entries used in the current frame are kept; unused entries from the
/// previous frame are discarded when `finish_frame()` swaps the frames.
pub struct LineLayoutCache {
    frames: [Mutex<FxHashMap<CacheKey, Arc<ShapedLine>>>; 2],
    current_idx: AtomicUsize,
}

impl LineLayoutCache {
    /// Create a new empty cache.
    pub fn new() -> Self {
        Self {
            frames: [
                Mutex::new(FxHashMap::default()),
                Mutex::new(FxHashMap::default()),
            ],
            current_idx: AtomicUsize::new(0),
        }
    }

    /// Get a cached line or shape a new one.
    ///
    /// If the line is cached in either frame, returns the cached `Arc`.
    /// If found in the previous frame, migrates to current frame.
    /// On miss, calls `shape_fn` and caches the result.
    pub fn get_or_shape<F>(&self, text: &str, font: FontHandle, shape_fn: F) -> Arc<ShapedLine>
    where
        F: FnOnce() -> ShapedLine,
    {
        let key = CacheKey {
            text_hash: hash_text(text),
            byte_len: text.len(),
            font,
        };

        let current = self.current_idx.load(Ordering::Acquire);
        let previous = 1 - current;

        // Check current frame first
        {
            let current_frame = self.frames[current].lock();
            if let Some(line) = current_frame.get(&key) {
                return Arc::clone(line);
            }
        }

        // Check previous frame and migrate if found
        {
            let previous_frame = self.frames[previous].lock();
            if let Some(line) = previous_frame.get(&key) {
                let line = Arc::clone(line);
                drop(previous_frame);
                self.frames[current].lock().insert(key, line.clone());
                return line;
            }
        }

        // Cache miss: shape and cache
        let line = Arc::new(shape_fn());
        self.frames[current].lock().insert(key, Arc::clone(&line));
        line
    }

    /// Get a cached line by text hash only (allocation-free lookup).
    ///
    /// Returns `None` if not cached. Does not migrate from previous frame.
    /// Use for retained-mode scenarios where the caller has a pre-computed hash.
    pub fn get_by_hash(
        &self,
        text_hash: u64,
        byte_len: usize,
        font: FontHandle,
    ) -> Option<Arc<ShapedLine>> {
        let key = CacheKey {
            text_hash,
            byte_len,
            font,
        };

        let current = self.current_idx.load(Ordering::Acquire);
        let previous = 1 - current;

        // Check current frame
        if let Some(line) = self.frames[current].lock().get(&key) {
            return Some(Arc::clone(line));
        }

        // Check previous frame
        if let Some(line) = self.frames[previous].lock().get(&key) {
            return Some(Arc::clone(line));
        }

        None
    }

    /// Advance to the next frame, clearing the upcoming frame's cache.
    ///
    /// Must not be called concurrently with `get_or_shape` on the same instance.
    /// The `TextBackend` is `!Send`, so in practice both are called from
    /// the same thread (the render thread).
    ///
    /// Swaps current/previous frames and clears the new current frame.
    /// Call at the end of each render frame.
    pub fn finish_frame(&self) {
        let current = self.current_idx.load(Ordering::Acquire);
        let next = 1 - current;

        // Clear the frame that will become current
        self.frames[next].lock().clear();

        // Swap frames
        self.current_idx.store(next, Ordering::Release);
    }

    /// Returns the number of cached entries in both frames.
    #[cfg(test)]
    pub fn len(&self) -> (usize, usize) {
        let current = self.current_idx.load(Ordering::Acquire);
        let previous = 1 - current;
        (
            self.frames[current].lock().len(),
            self.frames[previous].lock().len(),
        )
    }
}

impl Default for LineLayoutCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute a hash for text content.
pub fn hash_text(text: &str) -> u64 {
    let mut h = FxHasher::default();
    text.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShapedGlyph;
    use crate::types::FontId;

    fn make_font_handle() -> FontHandle {
        FontHandle::from_ptr_size_scale(0x1000 as *const (), 16.0, 1.0)
    }

    fn make_shaped_line(width: f32) -> ShapedLine {
        ShapedLine {
            glyphs: vec![ShapedGlyph {
                glyph_id: 1,
                font_id: FontId::PRIMARY,
                x_advance_lpx: width,
                x_offset_lpx: 0.0,
                y_offset_lpx: 0.0,
            }],
            width_lpx: width,
            ascent_lpx: 12.0,
            descent_lpx: -4.0,
            y_offset_lpx: 0.0,
        }
    }

    #[test]
    fn cache_hit_same_frame() {
        let cache = LineLayoutCache::new();
        let font = make_font_handle();
        let mut calls = 0;

        let line1 = cache.get_or_shape("hello", font, || {
            calls += 1;
            make_shaped_line(50.0)
        });

        let line2 = cache.get_or_shape("hello", font, || {
            calls += 1;
            make_shaped_line(50.0)
        });

        assert_eq!(calls, 1, "shape_fn should only be called once");
        assert!(Arc::ptr_eq(&line1, &line2), "should return same Arc");
    }

    #[test]
    fn cache_hit_after_frame_swap() {
        let cache = LineLayoutCache::new();
        let font = make_font_handle();
        let mut calls = 0;

        let line1 = cache.get_or_shape("hello", font, || {
            calls += 1;
            make_shaped_line(50.0)
        });

        cache.finish_frame();

        let line2 = cache.get_or_shape("hello", font, || {
            calls += 1;
            make_shaped_line(50.0)
        });

        assert_eq!(calls, 1, "should migrate from previous frame");
        assert!(Arc::ptr_eq(&line1, &line2), "should return same Arc");
    }

    #[test]
    fn cache_miss_on_text_change() {
        let cache = LineLayoutCache::new();
        let font = make_font_handle();
        let mut calls = 0;

        cache.get_or_shape("hello", font, || {
            calls += 1;
            make_shaped_line(50.0)
        });

        cache.get_or_shape("world", font, || {
            calls += 1;
            make_shaped_line(60.0)
        });

        assert_eq!(calls, 2, "different text should miss cache");
    }

    #[test]
    fn entries_expire_after_two_frames() {
        let cache = LineLayoutCache::new();
        let font = make_font_handle();
        let mut calls = 0;

        cache.get_or_shape("hello", font, || {
            calls += 1;
            make_shaped_line(50.0)
        });

        cache.finish_frame();
        cache.finish_frame();

        cache.get_or_shape("hello", font, || {
            calls += 1;
            make_shaped_line(50.0)
        });

        assert_eq!(calls, 2, "entry should expire after two unused frames");
    }

    #[test]
    fn get_by_hash_works() {
        let cache = LineLayoutCache::new();
        let font = make_font_handle();

        let text = "hello";
        let text_hash = hash_text(text);

        cache.get_or_shape(text, font, || make_shaped_line(50.0));

        let result = cache.get_by_hash(text_hash, text.len(), font);
        assert!(result.is_some(), "should find by hash");
    }
}
