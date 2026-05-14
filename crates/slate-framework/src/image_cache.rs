//! Image cache and observer for device-lost recovery.
//!
//! `ImageCache` holds uploaded image data keyed by content hash. The
//! `ImageSystemObserver` clears atlas allocations on device-lost while
//! preserving CPU-side pixel data for re-upload.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Weak;

use slate_renderer::RendererObserver;
use slate_renderer::atlas::{Atlas, AtlasAllocation};
use wgpu::Queue;

/// Cache for uploaded images, keyed by (content_hash, width, height).
///
/// Composite key guards against 64-bit hash collisions — dimensions provide
/// extra entropy so two different images with the same hash display correctly.
///
/// Entries survive device-lost: only `alloc` is cleared (by observer),
/// while pixels/dimensions remain for automatic re-upload.
pub(crate) struct ImageCache {
    /// Key: (content_hash, width, height) — composite for collision resistance
    entries: HashMap<(u64, u32, u32), ImageCacheEntry>,
    /// Tracks whether OOM warning has been logged (warn-once pattern).
    oom_warned: bool,
}

/// Single cached image entry.
pub(crate) struct ImageCacheEntry {
    /// CPU-side pixel data (survives device-lost).
    pixels: Vec<u8>,
    /// Atlas allocation (None after device-lost, until re-uploaded).
    alloc: Option<AtlasAllocation>,
}

impl ImageCache {
    /// Create a new empty image cache.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            oom_warned: false,
        }
    }

    /// Upload image to atlas if not already cached, returning the allocation.
    ///
    /// - Entry exists with valid `alloc` → return cached UV.
    /// - Entry exists with `alloc == None` (post-device-lost) → re-allocate + re-upload + return.
    /// - No entry → allocate + upload + insert + return.
    /// - OOM → return None (caller skips push_image, logs once).
    pub fn upload_if_needed(
        &mut self,
        content_hash: u64,
        pixels: &[u8],
        width: u32,
        height: u32,
        atlas: &mut Atlas,
        queue: &Queue,
    ) -> Option<AtlasAllocation> {
        // Composite key for collision resistance
        let key = (content_hash, width, height);

        // Check for existing entry
        if let Some(entry) = self.entries.get_mut(&key) {
            // Cache hit with valid allocation
            if let Some(alloc) = entry.alloc {
                atlas.touch(alloc.alloc_id);
                return Some(alloc);
            }

            // Cache hit but allocation cleared (post-device-lost) — re-upload
            debug_assert!(
                entry.pixels.len() == (width as usize) * (height as usize) * 4,
                "ImageCacheEntry pixel buffer size mismatch during re-upload"
            );
            match atlas.allocate(width, height) {
                Ok(alloc) => {
                    atlas.upload(queue, alloc.alloc_id, &entry.pixels);
                    entry.alloc = Some(alloc);
                    return Some(alloc);
                }
                Err(e) => {
                    if !self.oom_warned {
                        log::warn!(target: "slate::image", "atlas OOM during re-upload: {e}");
                        self.oom_warned = true;
                    }
                    return None;
                }
            }
        }

        // Cache miss — allocate, upload, insert
        match atlas.allocate(width, height) {
            Ok(alloc) => {
                atlas.upload(queue, alloc.alloc_id, pixels);
                self.entries.insert(
                    key,
                    ImageCacheEntry {
                        pixels: pixels.to_vec(),
                        alloc: Some(alloc),
                    },
                );
                Some(alloc)
            }
            Err(e) => {
                if !self.oom_warned {
                    log::warn!(target: "slate::image", "atlas OOM: {e}; skipping image");
                    self.oom_warned = true;
                }
                None
            }
        }
    }

    /// Clear all atlas allocations (called by observer on device-lost).
    /// Pixel data is preserved for automatic re-upload.
    pub fn clear_allocations(&mut self) {
        for entry in self.entries.values_mut() {
            entry.alloc = None;
        }
        // Reset OOM warning so it fires again if we hit OOM after recovery
        self.oom_warned = false;
    }

    /// Number of cached entries (for testing/debugging).
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Observer that clears ImageCache allocations on device recreation.
///
/// Registered with the Renderer; fires when device is successfully rebuilt.
/// Clears stale AtlasAllocations while preserving pixel data for re-upload.
pub(crate) struct ImageSystemObserver {
    inner: Weak<RefCell<ImageCache>>,
}

impl ImageSystemObserver {
    /// Create a new observer wrapping a weak ref to the image cache.
    pub fn new(inner: Weak<RefCell<ImageCache>>) -> Self {
        Self { inner }
    }
}

impl RendererObserver for ImageSystemObserver {
    fn on_renderer_recreated(&self, generation: u64) {
        log::debug!(target: "slate::device_lost",
            "ImageSystemObserver: clearing atlas allocations (gen={})", generation);
        if let Some(strong) = self.inner.upgrade() {
            strong.borrow_mut().clear_allocations();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_cache_is_empty() {
        let cache = ImageCache::new();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn clear_allocations_preserves_entries() {
        let mut cache = ImageCache::new();
        // Manually insert an entry for testing (composite key: hash, width, height)
        cache.entries.insert(
            (12345u64, 1u32, 1u32),
            ImageCacheEntry {
                pixels: vec![255u8; 4],
                alloc: None, // Would normally be Some(...)
            },
        );
        assert_eq!(cache.len(), 1);

        cache.clear_allocations();
        assert_eq!(cache.len(), 1); // Entry preserved
    }
}
