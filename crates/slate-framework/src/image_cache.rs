//! Image cache and observer for device-lost recovery.
//!
//! `ImageCache` holds uploaded image data keyed by content hash. The
//! `ImageSystemObserver` clears atlas allocations on device-lost while
//! preserving CPU-side pixel data for re-upload.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Weak;

use slate_renderer::atlas::{Atlas, AtlasAllocation};
use slate_renderer::{RendererObserver, allocate_image, pad_rgba_with_gutter};
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
            // Cache hit with a still-live allocation. Gate on the monotonic
            // token, not the AllocId: the atlas evicts off-screen slots and
            // etagere recycles the numeric AllocId, so a stale entry's id can
            // be reused by another image — sampling its pixels (wrong texture)
            // on scroll-back. A mismatched token means our slot was evicted;
            // fall through and re-upload.
            if let Some(alloc) = entry.alloc
                && atlas.is_live(alloc.alloc_id, alloc.token)
            {
                atlas.touch(alloc.alloc_id);
                return Some(alloc);
            }
            // Either never allocated (post-device-lost) or evicted — drop the
            // stale handle and re-upload from the preserved pixels.
            entry.alloc = None;

            debug_assert!(
                entry.pixels.len() == (width as usize) * (height as usize) * 4,
                "ImageCacheEntry pixel buffer size mismatch during re-upload"
            );
            match allocate_image(atlas, width, height) {
                Ok(alloc) => {
                    let padded = pad_rgba_with_gutter(&entry.pixels, width, height);
                    atlas.upload(queue, alloc.alloc_id, &padded);
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

        // Cache miss — allocate (with a 1-texel transparent gutter so linear
        // sampling never bleeds from a packed neighbour), upload, insert.
        match allocate_image(atlas, width, height) {
            Ok(alloc) => {
                let padded = pad_rgba_with_gutter(pixels, width, height);
                atlas.upload(queue, alloc.alloc_id, &padded);
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
    use slate_renderer::atlas::Format;
    use wgpu::{
        Backends, DeviceDescriptor, Features, Instance, InstanceDescriptor, Limits, MemoryHints,
        RequestAdapterOptions,
    };

    /// Headless device for the eviction test; returns `None` on CI runners with
    /// no adapter so the test skips cleanly (mirrors the renderer harness).
    fn headless() -> Option<(wgpu::Device, Queue)> {
        let instance = Instance::new(InstanceDescriptor {
            backends: Backends::PRIMARY,
            // (descriptor fields spelled out to match the renderer harness)
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: Default::default(),
            display: None,
        });
        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok()?;
        pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("image-cache-test-device"),
            required_features: Features::empty(),
            required_limits: Limits::downlevel_defaults(),
            memory_hints: MemoryHints::Performance,
            trace: wgpu::Trace::Off,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
        }))
        .ok()
    }

    /// A solid-color 256×256 RGBA image with the given red channel as a marker.
    fn solid_image(r: u8) -> Vec<u8> {
        let mut px = vec![0u8; 256 * 256 * 4];
        for chunk in px.chunks_exact_mut(4) {
            chunk[0] = r;
            chunk[3] = 255;
        }
        px
    }

    #[test]
    fn scroll_back_after_eviction_returns_a_live_reupload_not_stale_uv() {
        let Some((device, queue)) = headless() else {
            eprintln!("image_cache: no GPU adapter — skipping");
            return;
        };
        let mut atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);
        let mut cache = ImageCache::new();

        // Frame 1: upload image X and remember its allocation.
        atlas.begin_frame();
        let x_pixels = solid_image(10);
        let x0 = cache
            .upload_if_needed(0xA11CE, &x_pixels, 256, 256, &mut atlas, &queue)
            .expect("initial X upload");

        // Frame 2: X is now evictable (untouched this frame). Pack the page with
        // distinct images until eviction reclaims X's slot — a 258²-padded tile
        // packs 7×7=49 per page, so 60 distinct uploads guarantees eviction.
        atlas.begin_frame();
        for i in 0..60u64 {
            let _ = cache.upload_if_needed(
                0x1000 + i,
                &solid_image((i as u8).wrapping_add(20)),
                256,
                256,
                &mut atlas,
                &queue,
            );
        }
        assert!(
            !atlas.is_live(x0.alloc_id, x0.token),
            "test precondition: X's slot must have been evicted"
        );

        // Frame 3: scroll X back into view. The cache entry still holds the
        // stale Some(x0); the token gate must detect the eviction and re-upload.
        atlas.begin_frame();
        let x1 = cache
            .upload_if_needed(0xA11CE, &x_pixels, 256, 256, &mut atlas, &queue)
            .expect("scroll-back X re-upload");

        // Without the gate, the hit path would return the stale x0 unchanged.
        assert_ne!(
            x0.token, x1.token,
            "scroll-back must re-allocate (fresh token), not return the stale handle"
        );
        // The returned handle points at a genuinely live slot (its uv_rect is
        // the freshly re-uploaded slot, not a slot another image now owns).
        assert!(
            atlas.is_live(x1.alloc_id, x1.token),
            "re-uploaded X must be live"
        );
    }

    #[test]
    fn on_screen_image_hits_cache_without_reallocate() {
        let Some((device, queue)) = headless() else {
            return;
        };
        let mut atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);
        let mut cache = ImageCache::new();
        atlas.begin_frame();
        let px = solid_image(7);
        let a = cache
            .upload_if_needed(0xBEEF, &px, 256, 256, &mut atlas, &queue)
            .expect("first upload");
        // Same frame, re-request: must be a pure cache hit — same token, no
        // re-allocation.
        let b = cache
            .upload_if_needed(0xBEEF, &px, 256, 256, &mut atlas, &queue)
            .expect("cache hit");
        assert_eq!(a.token, b.token, "on-screen re-request must not re-allocate");
        assert_eq!(a.uv_rect, b.uv_rect);
    }

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
