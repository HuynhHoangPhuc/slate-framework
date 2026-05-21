//! Shared GPU texture atlas backing the glyph (Phase 5) and image (Phase 4)
//! pipelines.
//!
//! One [`Atlas`] wraps a single 2048² wgpu texture plus an
//! [`etagere::AtlasAllocator`] (shelf packing). Allocations return a
//! UV rectangle plus an opaque [`AllocId`] used for `touch` / `deallocate`
//! / `pin`.
//!
//! # Format
//!
//! - [`Format::R8Unorm`] — alpha mask (glyph atlas). No sRGB variant exists
//!   for R8 and alpha masks aren't gamma-encoded anyway.
//! - [`Format::Rgba8UnormSrgb`] — color (image atlas). Sampled values land
//!   in linear space, matching the surface's `Bgra8UnormSrgb` contract.
//!
//! # Usage flags (red-team P1-2)
//!
//! Texture is created with `COPY_DST | TEXTURE_BINDING | COPY_SRC` —
//! atlases are upload-only, never a render target. `COPY_SRC` enables
//! `copy_texture_to_buffer` readback for debug dumps and integration
//! tests; it does NOT make the atlas a render target.
//!
//! # Frame contract (red-team P1-7)
//!
//! Callers MUST invoke [`Atlas::begin_frame`] once per frame before
//! producing any draw work. Allocations whose `last_touch_frame` equals the
//! current frame counter are treated as "in use this frame" and are never
//! evicted by [`Atlas::allocate`]. Long-lived slots (e.g. the demo image)
//! should call [`Atlas::pin`] once at startup so they survive even when
//! they go untouched for a frame.
//!
//! # Spike findings
//!
//! `etagere` 0.2 (resolved 0.2.15) gives `Allocation { id, rectangle }` with
//! `rectangle.min.{x,y}` / `rectangle.max.{x,y}`. `etagere::AllocId` already
//! implements `Hash + Eq` (NonZeroU32 newtype), so we re-export it directly
//! rather than wrap.

use std::collections::{HashMap, HashSet, VecDeque};

use etagere::{AtlasAllocator, euclid::size2};
use wgpu::{
    Device, Extent3d, Origin3d, Queue, TexelCopyBufferLayout, TexelCopyTextureInfo, Texture,
    TextureAspect, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages, TextureView,
    TextureViewDescriptor,
};

/// Page side length in texels. Single-page-per-format hard cap for Phase 1
/// (multi-page deferred to framework Phase 2 — see plan §Non-Goals).
pub const PAGE_SIZE: u32 = 2048;

/// Re-exported `etagere::AllocId` — opaque, `Hash + Eq + Copy`.
pub type AllocId = etagere::AllocId;

/// Texel format for an atlas page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 1 byte/texel — alpha mask. Used by the glyph atlas.
    R8Unorm,
    /// 4 bytes/texel, sRGB-encoded — color. Used by the image atlas.
    Rgba8UnormSrgb,
}

impl Format {
    /// Bytes per texel for `Queue::write_texture` / `bytes_per_row` math.
    pub fn bytes_per_pixel(self) -> u32 {
        match self {
            Format::R8Unorm => 1,
            Format::Rgba8UnormSrgb => 4,
        }
    }

    /// Concrete `wgpu::TextureFormat`.
    pub fn wgpu_format(self) -> TextureFormat {
        match self {
            Format::R8Unorm => TextureFormat::R8Unorm,
            Format::Rgba8UnormSrgb => TextureFormat::Rgba8UnormSrgb,
        }
    }
}

/// Atlas allocation failure modes.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum AtlasError {
    /// Page exhausted even after LRU eviction.
    #[error("atlas page out of space")]
    OutOfSpace,
    /// Caller asked for a region larger than the page itself.
    #[error("requested {requested:?} exceeds atlas page size {max}")]
    TooLarge { requested: (u32, u32), max: u32 },
}

/// Successful allocation result. `uv_rect = [u_min, v_min, u_max, v_max]`
/// in `[0, 1]` page-relative coordinates.
///
/// `token` is a monotonic per-allocation stamp (never reused). It is the
/// sound liveness key: etagere recycles the numeric `AllocId` when an evicted
/// slot is reallocated, so `contains(alloc_id)` cannot tell a stale handle
/// from a fresh one — but a recycled slot always carries a newer `token`.
#[derive(Debug, Clone, Copy)]
pub struct AtlasAllocation {
    pub uv_rect: [f32; 4],
    pub alloc_id: AllocId,
    pub token: u64,
}

/// Per-allocation bookkeeping (private).
#[derive(Debug, Clone, Copy)]
struct AllocMeta {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    last_touch_frame: u64,
    token: u64,
}

/// VecDeque + position sidecar. `push_back` is O(1); `touch` / `pop_front`
/// / `remove` are O(n) on the deque-shift plus an O(n) sidecar fix-up.
/// Acceptable at atlas scale (Phase 1 caps at a few thousand live entries
/// per page); revisit with an intrusive doubly-linked list if profiling
/// shows churn cost in Phase 5 glyph workloads.
#[derive(Debug, Default)]
struct LruTracker {
    order: VecDeque<AllocId>,
    pos: HashMap<AllocId, usize>,
}

impl LruTracker {
    fn push_back(&mut self, id: AllocId) {
        self.pos.insert(id, self.order.len());
        self.order.push_back(id);
    }

    /// Move `id` to the most-recently-used end. O(n) — see struct docs.
    fn touch(&mut self, id: AllocId) {
        if let Some(&idx) = self.pos.get(&id) {
            self.order.remove(idx);
            // After remove, indices ≥ idx shifted down by one. Patch the
            // sidecar so future touches stay correct.
            for (i, &eid) in self.order.iter().enumerate().skip(idx) {
                self.pos.insert(eid, i);
            }
            self.pos.insert(id, self.order.len());
            self.order.push_back(id);
        }
    }

    fn pop_front(&mut self) -> Option<AllocId> {
        let id = self.order.pop_front()?;
        self.pos.remove(&id);
        for (i, &eid) in self.order.iter().enumerate() {
            self.pos.insert(eid, i);
        }
        Some(id)
    }

    fn remove(&mut self, id: AllocId) {
        if let Some(idx) = self.pos.remove(&id) {
            self.order.remove(idx);
            for (i, &eid) in self.order.iter().enumerate().skip(idx) {
                self.pos.insert(eid, i);
            }
        }
    }
}

/// Single-page GPU atlas backed by an etagere shelf allocator.
pub struct Atlas {
    texture: Texture,
    view: TextureView,
    allocator: AtlasAllocator,
    format: Format,
    page_size: u32,
    live: HashMap<AllocId, AllocMeta>,
    pinned: HashSet<AllocId>,
    lru: LruTracker,
    frame_counter: u64,
    /// Monotonic allocation stamp; never reused. See [`AtlasAllocation::token`].
    next_token: u64,
}

impl Atlas {
    /// Create a fresh `2048² (PAGE_SIZE)` atlas page.
    pub fn new(device: &Device, format: Format) -> Self {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some(match format {
                Format::R8Unorm => "slate-atlas-r8",
                Format::Rgba8UnormSrgb => "slate-atlas-rgba8-srgb",
            }),
            size: Extent3d {
                width: PAGE_SIZE,
                height: PAGE_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: format.wgpu_format(),
            // Upload-only: never a render target (red-team P1-2).
            // COPY_SRC is for readback / debug dumps, not rendering.
            usage: TextureUsages::COPY_DST
                | TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        let allocator = AtlasAllocator::new(size2(PAGE_SIZE as i32, PAGE_SIZE as i32));

        Self {
            texture,
            view,
            allocator,
            format,
            page_size: PAGE_SIZE,
            live: HashMap::new(),
            pinned: HashSet::new(),
            lru: LruTracker::default(),
            frame_counter: 0,
            // Start at 1 so 0 can mean "never allocated".
            next_token: 1,
        }
    }

    /// Increment the frame counter. Call once per frame before any draw
    /// work. Returns the new counter value.
    pub fn begin_frame(&mut self) -> u64 {
        self.frame_counter += 1;
        self.frame_counter
    }

    /// Try to allocate a `width × height` region. On `OutOfSpace`, runs LRU
    /// eviction once and retries.
    pub fn allocate(&mut self, width: u32, height: u32) -> Result<AtlasAllocation, AtlasError> {
        if width == 0 || height == 0 || width > self.page_size || height > self.page_size {
            return Err(AtlasError::TooLarge {
                requested: (width, height),
                max: self.page_size,
            });
        }

        if let Some(alloc) = self.try_allocate(width, height) {
            return Ok(alloc);
        }

        // Eviction path: free up roughly 1.5× the requested pixel area, then
        // retry exactly once.
        let needed = (width as u64) * (height as u64);
        self.evict_until_fits(needed.saturating_mul(3) / 2);

        self.try_allocate(width, height)
            .ok_or(AtlasError::OutOfSpace)
    }

    fn try_allocate(&mut self, width: u32, height: u32) -> Option<AtlasAllocation> {
        let alloc = self
            .allocator
            .allocate(size2(width as i32, height as i32))?;
        let r = alloc.rectangle;
        let x = r.min.x as u32;
        let y = r.min.y as u32;
        let id = alloc.id;
        let token = self.next_token;
        self.next_token += 1;
        self.live.insert(
            id,
            AllocMeta {
                x,
                y,
                width,
                height,
                last_touch_frame: self.frame_counter,
                token,
            },
        );
        self.lru.push_back(id);

        let page = self.page_size as f32;
        let uv_rect = [
            x as f32 / page,
            y as f32 / page,
            (x + width) as f32 / page,
            (y + height) as f32 / page,
        ];

        log::debug!(
            "atlas alloc {:?} at ({},{}) size {}×{}",
            id,
            x,
            y,
            width,
            height
        );

        Some(AtlasAllocation {
            uv_rect,
            alloc_id: id,
            token,
        })
    }

    /// Pin an allocation so LRU eviction never reclaims it. Idempotent.
    pub fn pin(&mut self, alloc_id: AllocId) {
        self.pinned.insert(alloc_id);
    }

    /// Mark `alloc_id` as touched this frame — eviction will skip it until
    /// `begin_frame()` advances the counter.
    pub fn touch(&mut self, alloc_id: AllocId) {
        if let Some(meta) = self.live.get_mut(&alloc_id) {
            meta.last_touch_frame = self.frame_counter;
            self.lru.touch(alloc_id);
        }
    }

    /// Return the slot to the shelf allocator and drop bookkeeping.
    pub fn deallocate(&mut self, alloc_id: AllocId) {
        if self.live.remove(&alloc_id).is_some() {
            self.allocator.deallocate(alloc_id);
            self.lru.remove(alloc_id);
            self.pinned.remove(&alloc_id);
            log::debug!("atlas dealloc {:?}", alloc_id);
        }
    }

    /// Upload `pixels` into the slot owned by `alloc_id`. `pixels.len()`
    /// must equal `width * height * bytes_per_pixel(format)`.
    ///
    /// `bytes_per_row` does NOT need 256-alignment when the source is a
    /// CPU slice — that contract only applies to `copy_buffer_to_texture`.
    pub fn upload(&self, queue: &Queue, alloc_id: AllocId, pixels: &[u8]) {
        let Some(meta) = self.live.get(&alloc_id) else {
            log::warn!("atlas upload: unknown alloc_id {alloc_id:?}");
            return;
        };
        let bpp = self.format.bytes_per_pixel();
        let expected = (meta.width * meta.height * bpp) as usize;
        debug_assert_eq!(
            pixels.len(),
            expected,
            "atlas upload size mismatch: got {} expected {}",
            pixels.len(),
            expected
        );

        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: Origin3d {
                    x: meta.x,
                    y: meta.y,
                    z: 0,
                },
                aspect: TextureAspect::All,
            },
            pixels,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(meta.width * bpp),
                rows_per_image: Some(meta.height),
            },
            Extent3d {
                width: meta.width,
                height: meta.height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// View into the atlas page texture (for sampler binding).
    pub fn texture_view(&self) -> &TextureView {
        &self.view
    }

    /// The atlas page texture itself. Exposed for `copy_texture_to_buffer`
    /// readback in tests; production code should bind via [`texture_view`].
    pub fn texture(&self) -> &Texture {
        &self.texture
    }

    /// Atlas pixel format.
    pub fn format(&self) -> Format {
        self.format
    }

    /// Normalized UV size of one texel on this atlas page.
    ///
    /// Returns `1.0 / page_side_length`. Used by [`crate::allocate_glyph`] to
    /// compute the 1-texel inset without coupling to the `PAGE_SIZE` constant
    /// directly — Phase 2 multi-page atlases can override this per page.
    pub fn texel_size_uv(&self) -> f32 {
        1.0 / self.page_size as f32
    }

    /// Whether `alloc_id` is currently live in the atlas.
    ///
    /// NOTE: insufficient as a liveness key for cached handles — etagere
    /// recycles the numeric `AllocId` on slot reuse, so this returns `true`
    /// for a *different* allocation that took over the slot. Use
    /// [`is_live`](Self::is_live) when validating a stored handle.
    pub fn contains(&self, alloc_id: AllocId) -> bool {
        self.live.contains_key(&alloc_id)
    }

    /// Whether the slot at `alloc_id` is still the *same* allocation that was
    /// handed out with `token`. Sound across eviction + slot reuse: a recycled
    /// `AllocId` carries a newer token, so a stale handle correctly reads dead.
    pub fn is_live(&self, alloc_id: AllocId, token: u64) -> bool {
        self.live.get(&alloc_id).is_some_and(|m| m.token == token)
    }

    /// Pop LRU front, evicting until at least `needed_pixels` of total
    /// allocated area has been freed OR the LRU is exhausted of evictable
    /// candidates. Touched-this-frame and pinned IDs are re-queued at the
    /// back rather than evicted.
    fn evict_until_fits(&mut self, needed_pixels: u64) {
        let mut freed: u64 = 0;
        // Bound the loop by the LRU size to avoid infinite re-queueing.
        let mut budget = self.lru.order.len();

        while budget > 0 && freed < needed_pixels {
            budget -= 1;
            let Some(id) = self.lru.pop_front() else {
                break;
            };
            let meta = match self.live.get(&id) {
                Some(m) => *m,
                None => continue,
            };
            let touched_now = meta.last_touch_frame == self.frame_counter;
            if touched_now || self.pinned.contains(&id) {
                self.lru.push_back(id);
                continue;
            }

            self.live.remove(&id);
            self.allocator.deallocate(id);
            freed += (meta.width as u64) * (meta.height as u64);
            log::debug!(
                "atlas evict {:?} (freed {}×{}={} px, total freed {})",
                id,
                meta.width,
                meta.height,
                meta.width * meta.height,
                freed
            );
        }
    }

    /// Number of live allocations (testing / debug introspection).
    pub fn live_count(&self) -> usize {
        self.live.len()
    }

    /// Sum of currently allocated pixel area (testing / debug).
    pub fn allocated_pixels(&self) -> u64 {
        self.live
            .values()
            .map(|m| (m.width as u64) * (m.height as u64))
            .sum()
    }
}
