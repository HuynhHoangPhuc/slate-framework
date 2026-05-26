//! Regression: a `GlyphCache` is paired one-to-one with the `Atlas` it
//! materialized into. Reusing one cache across two atlases (the pre-fix
//! multi-window shape) is unsafe: cached `AllocId + token` references are
//! atlas-scoped, so the second atlas overwrites the cache entry and the first
//! atlas's next paint either misses (re-materialize race) or serves pixels
//! that belong to the wrong glyph slot — visible as text corruption when
//! multiple windows paint concurrently.
//!
//! The fix moves `GlyphCache` (and `ImageCache`) into `WindowState` so the
//! cache lifetime equals the atlas lifetime. This test pins the underlying
//! invariant at the primitive level: an `AllocId` minted by atlas A cannot be
//! served from atlas B, and the `live_hit` gate must force re-materialization
//! when an entry's atlas-of-origin differs from the atlas being queried.

mod common;

use common::mock::MockBackend;
use slate_renderer::atlas::Atlas;
use slate_text::glyph_cache::GlyphCache;
use slate_text::{Font, TextBackend};

#[test]
fn cache_entry_from_one_atlas_is_not_served_by_another() {
    let Some((device, queue)) = common::headless::make_headless_device() else {
        eprintln!("Skipping test: no GPU adapter available");
        return;
    };

    let mut atlas_a = Atlas::new(&device, slate_renderer::atlas::Format::R8Unorm);
    let mut atlas_b = Atlas::new(&device, slate_renderer::atlas::Format::R8Unorm);

    // Single shared cache — the pre-fix shape, only here to PROVE the cross-
    // atlas hazard. Production code now keeps one cache per atlas.
    let mut cache = GlyphCache::new();

    let backend = MockBackend::default();
    let mut font_backend = MockBackend::default();
    let font = font_backend.load_font("Test", 16.0, 1.0).unwrap();
    let fh = font.handle();
    let shaped = backend.shape_line(&font, "a").unwrap();
    let gid = shaped.glyphs[0].glyph_id;

    // Materialize G via atlas A. Cache now points to atlas A's slot.
    atlas_a.begin_frame();
    let was_miss_a = cache
        .materialize(&backend, &font, gid, 0, &mut atlas_a, &queue)
        .unwrap();
    assert!(was_miss_a, "first materialize into atlas A must be a miss");
    let entry_a = *cache.get(fh, gid, 0).unwrap();
    assert!(
        atlas_a.is_live(entry_a.alloc.alloc_id, entry_a.alloc.token),
        "freshly materialized slot must be live in atlas A",
    );

    // Materialize the SAME glyph against atlas B. live_hit rejects the
    // atlas-A entry (its AllocId+token belongs to atlas A's slot table), so
    // the single cache slot gets overwritten in-place with atlas B's
    // allocation handle. This is the smoking gun: a GlyphCache can only
    // track one atlas's bookkeeping at a time, even when both atlases happen
    // to mint structurally identical handles for the first allocation.
    atlas_b.begin_frame();
    let was_miss_b = cache
        .materialize(&backend, &font, gid, 0, &mut atlas_b, &queue)
        .unwrap();
    assert!(
        was_miss_b,
        "materialize against a *different* atlas must re-rasterize, not reuse the atlas-A entry",
    );
    let entry_b = *cache.get(fh, gid, 0).unwrap();
    assert!(
        atlas_b.is_live(entry_b.alloc.alloc_id, entry_b.alloc.token),
        "after re-materialize, cache entry must point at a live slot in atlas B",
    );
    // Atlas A's slot was never deallocated — it's still live on the GPU,
    // holding the pixels that atlas A's render pass expects to sample. The
    // cache has simply forgotten about it: a single (FontHandle, glyph_id,
    // variant) key can map to at most one allocation, so atlas A's handle
    // is overwritten the moment atlas B claims the slot.
    assert!(
        atlas_a.is_live(entry_a.alloc.alloc_id, entry_a.alloc.token),
        "atlas A's slot is still GPU-live; the cache just lost the handle",
    );
}

#[test]
fn separate_caches_paired_with_separate_atlases_do_not_interfere() {
    // Mirror of the post-fix architecture: each (Atlas, GlyphCache) pair is
    // independent. Materializing the same glyph in both pairs must produce
    // two stable, independently-live cache entries — neither pair perturbs
    // the other.
    let Some((device, queue)) = common::headless::make_headless_device() else {
        eprintln!("Skipping test: no GPU adapter available");
        return;
    };

    let mut atlas_a = Atlas::new(&device, slate_renderer::atlas::Format::R8Unorm);
    let mut atlas_b = Atlas::new(&device, slate_renderer::atlas::Format::R8Unorm);
    let mut cache_a = GlyphCache::new();
    let mut cache_b = GlyphCache::new();

    let backend = MockBackend::default();
    let mut font_backend = MockBackend::default();
    let font = font_backend.load_font("Test", 16.0, 1.0).unwrap();
    let fh = font.handle();
    let shaped = backend.shape_line(&font, "a").unwrap();
    let gid = shaped.glyphs[0].glyph_id;

    atlas_a.begin_frame();
    cache_a
        .materialize(&backend, &font, gid, 0, &mut atlas_a, &queue)
        .unwrap();
    let entry_a = *cache_a.get(fh, gid, 0).unwrap();

    atlas_b.begin_frame();
    cache_b
        .materialize(&backend, &font, gid, 0, &mut atlas_b, &queue)
        .unwrap();
    let entry_b = *cache_b.get(fh, gid, 0).unwrap();

    // Both entries live in their own atlas; neither is recognized by the other.
    assert!(atlas_a.is_live(entry_a.alloc.alloc_id, entry_a.alloc.token));
    assert!(atlas_b.is_live(entry_b.alloc.alloc_id, entry_b.alloc.token));

    // Re-querying cache A in a second frame is a pure hit — cache B's
    // independent materialize did not perturb cache A's bookkeeping.
    atlas_a.begin_frame();
    let was_miss = cache_a
        .materialize(&backend, &font, gid, 0, &mut atlas_a, &queue)
        .unwrap();
    assert!(
        !was_miss,
        "per-atlas cache must remain a hit across frames when its own atlas slot stays live",
    );
    let entry_a2 = *cache_a.get(fh, gid, 0).unwrap();
    assert_eq!(
        entry_a.alloc.alloc_id, entry_a2.alloc.alloc_id,
        "stable cache entry — no re-allocation",
    );
    assert_eq!(entry_a.alloc.token, entry_a2.alloc.token);
}
