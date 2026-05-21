//! Regression: a glyph whose atlas slot is evicted (while its CPU `CachedGlyph`
//! survives) must be re-materialized on re-request, not served with a stale
//! `uv_rect`. Mirrors the image-cache `scroll_back_after_eviction` test.
//!
//! The shared `MockBackend` caps glyph bitmaps at 16×16 — far too small to fill
//! a 2048² atlas page cheaply. This file defines a local `BigGlyphBackend` whose
//! glyphs are ~600×600 (602² padded → 3×3 = 9 per page), so ~12 distinct
//! allocations overflow the page and force atlas-LRU eviction while staying well
//! below the cache's `max_entries=8192` full-drain threshold — isolating the
//! dangerous atlas-LRU path the full-drain would otherwise pre-empt.

mod common;

use slate_renderer::atlas::Atlas;
use slate_text::error::TextError;
use slate_text::font_handle::FontHandle;
use slate_text::glyph_cache::GlyphCache;
use slate_text::types::{
    FontDescriptor, FontId, FontMetrics, GlyphBitmap, GlyphBounds, ShapedGlyph, ShapedLine,
};
use slate_text::{Font, TextBackend};

/// Side length of each glyph bitmap. 600² padded to 602² packs 3×3=9 per
/// 2048² page; 12 distinct glyphs guarantee overflow → atlas eviction.
const GLYPH_SIDE: u32 = 600;

// ── BigFont ────────────────────────────────────────────────────────────────

struct BigFont {
    handle: FontHandle,
    size_lpx: f32,
    scale: f32,
}

impl Font for BigFont {
    fn handle(&self) -> FontHandle {
        self.handle
    }
    fn metrics(&self) -> FontMetrics {
        FontMetrics {
            ascent_lpx: 12.0,
            descent_lpx: -3.0,
            line_gap_lpx: 1.0,
            x_height_lpx: 8.0,
            cap_height_lpx: 10.0,
            units_per_em: 2048,
        }
    }
    fn size_lpx(&self) -> f32 {
        self.size_lpx
    }
    fn scale(&self) -> f32 {
        self.scale
    }
}

// ── BigGlyphBackend ──────────────────────────────────────────────────────────

/// Backend that rasterizes every glyph as a `GLYPH_SIDE²` alpha bitmap, large
/// enough to fill an atlas page in a handful of allocations.
struct BigGlyphBackend;

impl TextBackend for BigGlyphBackend {
    type Font = BigFont;

    fn load_font(
        &mut self,
        _family: &str,
        size_lpx: f32,
        scale: f32,
    ) -> Result<Self::Font, TextError> {
        Ok(BigFont {
            handle: FontHandle::from_face_id(0xB16F0117, size_lpx, scale),
            size_lpx,
            scale,
        })
    }

    fn load_font_from_bytes(
        &mut self,
        bytes: &'static [u8],
        size_lpx: f32,
        scale: f32,
    ) -> Result<Self::Font, TextError> {
        Ok(BigFont {
            handle: FontHandle::from_face_id(bytes.as_ptr() as u64, size_lpx, scale),
            size_lpx,
            scale,
        })
    }

    fn shape_line(&self, _font: &Self::Font, text: &str) -> Result<ShapedLine, TextError> {
        let glyphs: Vec<ShapedGlyph> = text
            .chars()
            .enumerate()
            .map(|(i, _)| ShapedGlyph {
                glyph_id: i as u32 + 1,
                font_id: FontId::PRIMARY,
                font_handle: Default::default(),
                x_advance_lpx: GLYPH_SIDE as f32,
                position_lpx: [i as f32 * GLYPH_SIDE as f32, 0.0],
                cluster: 0,
            })
            .collect();
        let width = glyphs.len() as f32 * GLYPH_SIDE as f32;
        Ok(ShapedLine {
            glyphs,
            width_lpx: width,
            ascent_lpx: 12.0,
            descent_lpx: -3.0,
            y_offset_lpx: 0.0,
        })
    }

    fn rasterize_glyph(
        &self,
        _font: &Self::Font,
        _glyph_id: u32,
        _variant: u8,
    ) -> Result<GlyphBitmap, TextError> {
        let side = GLYPH_SIDE;
        Ok(GlyphBitmap {
            width: side,
            height: side,
            bearing_x_lpx: 1.0,
            bearing_y_lpx: (side as f32) - 1.0,
            advance_x_lpx: (side as f32) + 2.0,
            alpha: vec![0xFF; (side * side) as usize],
        })
    }

    fn glyph_raster_bounds(
        &self,
        _font: &Self::Font,
        _glyph_id: u32,
    ) -> Result<GlyphBounds, TextError> {
        Ok(GlyphBounds {
            width: GLYPH_SIDE,
            height: GLYPH_SIDE,
        })
    }

    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError> {
        Ok(vec![])
    }
}

#[test]
fn glyph_evicted_offscreen_then_requested_returns_live_reupload_not_stale_uv() {
    let Some((device, queue)) = common::headless::make_headless_device() else {
        eprintln!("Skipping test: no GPU adapter available");
        return;
    };

    let mut atlas = Atlas::new(&device, slate_renderer::atlas::Format::R8Unorm);
    let mut cache = GlyphCache::new();
    let mut backend = BigGlyphBackend;
    let font = backend.load_font("Big", 16.0, 1.0).unwrap();
    let fh = font.handle();

    // Frame 1: materialize glyph G (id=1) and remember its allocation.
    atlas.begin_frame();
    cache
        .materialize(&backend, &font, 1, 0, &mut atlas, &queue)
        .unwrap();
    let g0 = cache.get(fh, 1, 0).expect("G cached after first frame").alloc;

    // Frame 2: G is now evictable (untouched this frame). Pack the page with
    // distinct glyphs (ids 2..=13) until eviction reclaims G's slot. 9 glyphs
    // fill a page, so 12 distinct allocations guarantee eviction — all below
    // max_entries=8192, so the cache's safe full-drain never fires.
    atlas.begin_frame();
    for id in 2u32..=13 {
        // Once the page is full and every slot was touched this frame, further
        // allocations OOM — but G (untouched this frame) is reclaimed before
        // that point, which is all this loop needs. Tolerate the OOM like the
        // image-cache sibling test does.
        let _ = cache.materialize(&backend, &font, id, 0, &mut atlas, &queue);
    }
    assert!(
        !atlas.is_live(g0.alloc_id, g0.token),
        "test precondition: G's atlas slot must have been evicted"
    );

    // Frame 3: re-request G. The CPU entry still holds the stale g0; the
    // liveness gate must detect the eviction and re-materialize.
    atlas.begin_frame();
    cache
        .materialize(&backend, &font, 1, 0, &mut atlas, &queue)
        .unwrap();
    let g1 = cache.get(fh, 1, 0).expect("G re-requested in frame 3").alloc;

    // Without the gate, the contains_key hit returns Ok(false) and get() serves
    // the stale g0 unchanged.
    assert_ne!(
        g0.token, g1.token,
        "re-request must re-allocate (fresh token), not return the stale handle"
    );
    assert!(
        atlas.is_live(g1.alloc_id, g1.token),
        "re-materialized G must point at a genuinely live slot"
    );
}

#[test]
fn on_screen_glyph_hits_cache_without_reallocate() {
    let Some((device, queue)) = common::headless::make_headless_device() else {
        eprintln!("Skipping test: no GPU adapter available");
        return;
    };

    let mut atlas = Atlas::new(&device, slate_renderer::atlas::Format::R8Unorm);
    let mut cache = GlyphCache::new();
    let mut backend = BigGlyphBackend;
    let font = backend.load_font("Big", 16.0, 1.0).unwrap();
    let fh = font.handle();

    atlas.begin_frame();
    let was_miss = cache
        .materialize(&backend, &font, 1, 0, &mut atlas, &queue)
        .unwrap();
    assert!(was_miss, "first materialize is a miss");
    let a = cache.get(fh, 1, 0).unwrap().alloc;

    // Same frame, re-request: a live hit must stay a pure cache hit — same
    // token, materialize returns Ok(false), no re-allocation.
    let was_miss = cache
        .materialize(&backend, &font, 1, 0, &mut atlas, &queue)
        .unwrap();
    assert!(!was_miss, "live re-request must be a cache hit, not a re-raster");
    let b = cache.get(fh, 1, 0).unwrap().alloc;
    assert_eq!(a.token, b.token, "on-screen re-request must not re-allocate");
    assert_eq!(a.uv_rect, b.uv_rect);
}
