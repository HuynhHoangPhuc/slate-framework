//! Glyph cache for atlas-backed text rendering.
//!
//! `GlyphCache` rasterizes shaped glyphs and uploads them to the GPU atlas.
//! Cache key is `(FontHandle, glyph_id, variant)`.

use std::collections::HashMap;

use slate_renderer::atlas::{Atlas, AtlasAllocation};
use slate_renderer::glyph_pipeline::allocate_glyph;

use crate::backend::{Font, TextBackend};
use crate::error::TextError;
use crate::font_handle::FontHandle;
use crate::types::{CachedGlyph, GlyphBitmap, GlyphMetrics, ShapedLine};

/// Pool for reusing alpha buffers during rasterization.
///
/// Reduces allocations for dynamic text (e.g., FPS counters, live data).
/// Buffers under 64KB are retained; larger ones are dropped to avoid memory bloat.
pub struct PendingUploadPool {
    buffers: Vec<Vec<u8>>,
}

impl PendingUploadPool {
    /// Creates a new empty buffer pool.
    pub fn new() -> Self {
        Self {
            buffers: Vec::new(),
        }
    }

    /// Acquires a buffer with at least the given capacity.
    ///
    /// Reuses a pooled buffer if available; otherwise allocates a new one.
    pub fn acquire(&mut self, capacity: usize) -> Vec<u8> {
        self.buffers
            .pop()
            .map(|mut b| {
                b.clear();
                if b.capacity() < capacity {
                    b.reserve(capacity - b.capacity());
                }
                b
            })
            .unwrap_or_else(|| Vec::with_capacity(capacity))
    }

    /// Returns a buffer to the pool for reuse.
    ///
    /// Buffers over 64KB are dropped to avoid memory bloat.
    pub fn release(&mut self, buf: Vec<u8>) {
        if buf.capacity() < 64 * 1024 {
            self.buffers.push(buf);
        }
    }

    /// Returns the number of pooled buffers.
    pub fn len(&self) -> usize {
        self.buffers.len()
    }

    /// Returns true if the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty()
    }
}

impl Default for PendingUploadPool {
    fn default() -> Self {
        Self::new()
    }
}

/// Pending glyph upload waiting for atlas allocation.
pub(crate) struct PendingUpload {
    pub font_handle: FontHandle,
    pub glyph_id: u32,
    pub variant: u8,
    pub bitmap: GlyphBitmap,
}

impl PendingUpload {
    /// Checks if this pending upload matches the given cache key.
    #[inline]
    pub fn matches(&self, key: &(FontHandle, u32, u8)) -> bool {
        self.font_handle == key.0 && self.glyph_id == key.1 && self.variant == key.2
    }
}

/// Cache of rasterized glyphs backed by a GPU atlas.
///
/// # Usage
///
/// 1. Call `materialize()` with shaped text to rasterize missing glyphs
/// 2. Call `flush()` to upload pending glyphs to the atlas
/// 3. Call `get()` to retrieve cached glyphs for rendering
///
/// # Cache Key
///
/// Glyphs are keyed by `(FontHandle, glyph_id, variant)` where:
/// - `FontHandle` encodes font pointer, size, and scale
/// - `glyph_id` is the glyph index in the font
/// - `variant` is the sub-pixel X offset (0-3)
pub struct GlyphCache {
    cache: HashMap<(FontHandle, u32, u8), CachedGlyph>,
    pending: Vec<PendingUpload>,
}

impl GlyphCache {
    /// Creates a new empty glyph cache.
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            pending: Vec::new(),
        }
    }

    /// Looks up a cached glyph by font handle, glyph ID, and variant.
    pub fn get(&self, font: FontHandle, glyph_id: u32, variant: u8) -> Option<&CachedGlyph> {
        self.cache.get(&(font, glyph_id, variant))
    }

    /// Rasterizes a single glyph variant if not already cached.
    ///
    /// Preferred over `materialize()` for lazy per-variant rasterization.
    /// Queues the rasterized bitmap for upload; call `flush()` to commit to atlas.
    ///
    /// Returns `true` if the glyph was rasterized and queued (cache miss),
    /// `false` if already cached or pending.
    pub fn materialize_one<B: TextBackend>(
        &mut self,
        backend: &B,
        font: &B::Font,
        glyph_id: u32,
        variant: u8,
    ) -> Result<bool, TextError> {
        let key = (font.handle(), glyph_id, variant);

        if self.cache.contains_key(&key) {
            return Ok(false);
        }
        if self.pending.iter().any(|p| p.matches(&key)) {
            return Ok(false);
        }

        let bitmap = backend.rasterize_glyph(font, glyph_id, variant)?;
        if bitmap.width == 0 || bitmap.height == 0 {
            return Ok(false);
        }

        self.pending.push(PendingUpload {
            font_handle: key.0,
            glyph_id,
            variant,
            bitmap,
        });
        Ok(true)
    }

    /// Rasterizes any missing glyphs from the shaped line.
    ///
    /// Queues rasterized bitmaps for upload; call `flush()` to commit to atlas.
    /// Eagerly rasterizes all 4 sub-pixel variants per glyph.
    ///
    /// Uses bounds-check-before-rasterize pattern: queries bounds first (cheap),
    /// then only rasterizes if non-zero. Whitespace glyphs are skipped efficiently.
    ///
    /// # Note
    ///
    /// Consider using `materialize_one()` for lazy per-variant rasterization
    /// (4× fewer rasterizations for static text).
    #[deprecated(note = "use materialize_one() for lazy per-variant rasterization")]
    pub fn materialize<B: TextBackend>(
        &mut self,
        backend: &B,
        font: &B::Font,
        shaped: &ShapedLine,
    ) -> Result<(), TextError> {
        let fh = font.handle();
        for g in &shaped.glyphs {
            let bounds = backend.glyph_raster_bounds(font, g.glyph_id)?;
            if bounds.is_whitespace() {
                continue;
            }

            for v in 0u8..4 {
                let key = (fh, g.glyph_id, v);
                if self.cache.contains_key(&key) {
                    continue;
                }
                if self
                    .pending
                    .iter()
                    .any(|p| p.font_handle == key.0 && p.glyph_id == key.1 && p.variant == key.2)
                {
                    continue;
                }
                let bitmap = backend.rasterize_glyph(font, g.glyph_id, v)?;
                if bitmap.width == 0 || bitmap.height == 0 {
                    log::warn!(
                        "glyph {} had non-zero bounds but rasterized to empty bitmap",
                        g.glyph_id
                    );
                    continue;
                }
                self.pending.push(PendingUpload {
                    font_handle: fh,
                    glyph_id: g.glyph_id,
                    variant: v,
                    bitmap,
                });
            }
        }
        debug_assert!(
            self.pending.len() < 65_536,
            "pending queue too large — caller may have forgotten to flush"
        );
        Ok(())
    }

    /// Uploads pending glyphs to the atlas.
    ///
    /// Drains pending uploads, allocates atlas slots, builds padded buffers
    /// with 1-pixel zero-fill gutter, and commits to GPU.
    pub fn flush(&mut self, atlas: &mut Atlas, queue: &wgpu::Queue) -> Result<(), TextError> {
        for p in self.pending.drain(..) {
            let (alloc_id, uv_rect) = allocate_glyph(atlas, p.bitmap.width, p.bitmap.height)
                .map_err(|e| TextError::RasterizationFailed(format!("atlas alloc: {e:?}")))?;

            let padded = pad_with_gutter(&p.bitmap.alpha, p.bitmap.width, p.bitmap.height);
            atlas.upload(queue, alloc_id, &padded);

            let metrics = GlyphMetrics::from_bitmap(&p.bitmap);
            self.cache.insert(
                (p.font_handle, p.glyph_id, p.variant),
                CachedGlyph {
                    alloc: AtlasAllocation { uv_rect, alloc_id },
                    metrics,
                },
            );
        }
        Ok(())
    }

    /// Marks a glyph as recently used for LRU tracking.
    ///
    /// Call this for each glyph rendered to prevent atlas eviction.
    pub fn touch(&mut self, atlas: &mut Atlas, font: FontHandle, glyph_id: u32, variant: u8) {
        if let Some(cg) = self.cache.get(&(font, glyph_id, variant)) {
            atlas.touch(cg.alloc.alloc_id);
        }
    }

    /// Clears all cached glyphs and pending uploads.
    pub fn clear(&mut self) {
        self.cache.clear();
        self.pending.clear();
    }

    /// Returns the number of pending uploads.
    ///
    /// Useful for testing and debugging.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Returns the number of cached glyphs.
    ///
    /// Useful for testing and debugging.
    pub fn cache_len(&self) -> usize {
        self.cache.len()
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
