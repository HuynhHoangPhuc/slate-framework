//! Integration test: GlyphCache with real wgpu atlas.

mod common;

use common::mock::MockBackend;
use slate_renderer::atlas::Atlas;
use slate_text::glyph_cache::GlyphCache;
use slate_text::{Font, TextBackend};

#[test]
fn cache_roundtrip_with_atlas() {
    let Some((device, queue)) = common::headless::make_headless_device() else {
        eprintln!("Skipping test: no GPU adapter available");
        return;
    };

    let mut atlas = Atlas::new(&device, slate_renderer::atlas::Format::R8Unorm);
    let mut cache = GlyphCache::new();
    let backend = MockBackend::default();
    let mut font_backend = MockBackend::default();
    let font = font_backend.load_font("Test", 16.0, 1.0).unwrap();
    let shaped = backend.shape_line(&font, "abc").unwrap();

    let fh = font.handle();

    // Materialize each glyph/variant immediately
    for g in &shaped.glyphs {
        for v in 0u8..4 {
            cache
                .materialize(&backend, &font, g.glyph_id, v, &mut atlas, &queue)
                .unwrap();
        }
    }

    // After immediate upload: cache hit
    for g in &shaped.glyphs {
        for v in 0u8..4 {
            let cached = cache
                .get(fh, g.glyph_id, v)
                .expect("glyph should be cached after immediate upload");
            assert_eq!(cached.metrics.width, g.glyph_id.clamp(1, 16));
            assert_eq!(cached.metrics.height, v as u32 + 1);
            assert_eq!(cached.metrics.bearing_x_lpx, 1.0);
            assert_eq!(
                cached.metrics.advance_x_lpx,
                (g.glyph_id.clamp(1, 16) as f32) + 2.0
            );
        }
    }

    // Second call returns false (already cached)
    for g in &shaped.glyphs {
        let was_miss = cache
            .materialize(&backend, &font, g.glyph_id, 0, &mut atlas, &queue)
            .unwrap();
        assert!(!was_miss, "should be cache hit on second call");
    }

    // Touch should not panic
    cache.touch(&mut atlas, fh, 1, 0);
}

#[test]
fn second_get_returns_same_alloc_id() {
    let Some((device, queue)) = common::headless::make_headless_device() else {
        eprintln!("Skipping test: no GPU adapter available");
        return;
    };

    let mut atlas = Atlas::new(&device, slate_renderer::atlas::Format::R8Unorm);
    let mut cache = GlyphCache::new();
    let backend = MockBackend::default();
    let mut font_backend = MockBackend::default();
    let font = font_backend.load_font("Test", 16.0, 1.0).unwrap();
    let shaped = backend.shape_line(&font, "a").unwrap();

    let fh = font.handle();
    let gid = shaped.glyphs[0].glyph_id;

    cache
        .materialize(&backend, &font, gid, 0, &mut atlas, &queue)
        .unwrap();

    let first = cache.get(fh, gid, 0).unwrap();
    let first_id = first.alloc.alloc_id;

    let second = cache.get(fh, gid, 0).unwrap();
    assert_eq!(
        first_id, second.alloc.alloc_id,
        "same key should return same alloc_id"
    );
}
