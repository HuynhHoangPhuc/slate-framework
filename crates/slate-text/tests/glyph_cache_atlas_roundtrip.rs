//! Integration test: GlyphCache with real wgpu atlas.

mod common;

use slate_renderer::atlas::Atlas;
use slate_text::backend::{Font, TextBackend};
use slate_text::error::TextError;
use slate_text::font_handle::FontHandle;
use slate_text::glyph_cache::GlyphCache;
use slate_text::types::{
    FontDescriptor, FontId, FontMetrics, GlyphBitmap, GlyphBounds, ShapedGlyph, ShapedLine,
};

/// Mock font for testing.
struct MockFont {
    handle: FontHandle,
    size_lpx: f32,
    scale: f32,
}

impl Font for MockFont {
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

/// Mock backend producing predictable bitmaps.
struct MockBackend;

impl TextBackend for MockBackend {
    type Font = MockFont;

    fn load_font(
        &mut self,
        _family: &str,
        size_lpx: f32,
        scale: f32,
    ) -> Result<Self::Font, TextError> {
        let ptr = 0x12345678 as *const u8;
        Ok(MockFont {
            handle: FontHandle::from_ptr_size_scale(ptr, size_lpx, scale),
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
        Ok(MockFont {
            handle: FontHandle::from_ptr_size_scale(bytes.as_ptr(), size_lpx, scale),
            size_lpx,
            scale,
        })
    }

    fn shape_line(&self, _font: &Self::Font, text: &str) -> Result<ShapedLine, TextError> {
        let glyphs: Vec<ShapedGlyph> = text
            .chars()
            .enumerate()
            .map(|(i, _)| ShapedGlyph {
                glyph_id: i as u32 + 1, // Start at 1 to avoid 0-size bitmaps
                font_id: FontId::PRIMARY,
                x_advance_lpx: 10.0,
                x_offset_lpx: 0.0,
                y_offset_lpx: 0.0,
            })
            .collect();
        let width = glyphs.len() as f32 * 10.0;
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
        glyph_id: u32,
        variant: u8,
    ) -> Result<GlyphBitmap, TextError> {
        // Predictable bitmap: width = glyph_id, height = variant + 1
        let w = glyph_id.min(16).max(1);
        let h = (variant as u32 + 1).min(16);
        let alpha = vec![0xFF; (w * h) as usize];
        Ok(GlyphBitmap {
            width: w,
            height: h,
            bearing_x_lpx: 1.0,
            bearing_y_lpx: (h as f32) - 1.0,
            advance_x_lpx: (w as f32) + 2.0,
            alpha,
        })
    }

    fn glyph_raster_bounds(
        &self,
        _font: &Self::Font,
        glyph_id: u32,
    ) -> Result<GlyphBounds, TextError> {
        let w = glyph_id.min(16).max(1);
        Ok(GlyphBounds {
            width: w,
            height: 4,
        })
    }

    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError> {
        Ok(vec![])
    }
}

#[test]
fn cache_roundtrip_with_atlas() {
    let Some((device, queue)) = common::headless::make_headless_device() else {
        eprintln!("Skipping test: no GPU adapter available");
        return;
    };

    let mut atlas = Atlas::new(&device, slate_renderer::atlas::Format::R8Unorm);
    let mut cache = GlyphCache::new();
    let mut backend = MockBackend;
    let font = backend.load_font("Test", 16.0, 1.0).unwrap();
    let shaped = backend.shape_line(&font, "abc").unwrap();

    // Materialize glyphs
    cache.materialize(&backend, &font, &shaped).unwrap();
    assert!(cache.pending_len() > 0, "should have pending uploads");

    // Before flush: cache miss
    assert!(
        cache.get(font.handle(), 1, 0).is_none(),
        "should not be cached before flush"
    );

    // Flush to atlas
    cache.flush(&mut atlas, &queue).unwrap();
    assert_eq!(
        cache.pending_len(),
        0,
        "pending should be empty after flush"
    );

    // After flush: cache hit
    let fh = font.handle();
    for g in &shaped.glyphs {
        for v in 0u8..4 {
            let cached = cache
                .get(fh, g.glyph_id, v)
                .expect("glyph should be cached after flush");
            // Verify metrics match rasterization
            assert_eq!(cached.metrics.width, g.glyph_id.min(16).max(1));
            assert_eq!(cached.metrics.height, v as u32 + 1);
            assert_eq!(cached.metrics.bearing_x_lpx, 1.0);
            assert_eq!(
                cached.metrics.advance_x_lpx,
                (g.glyph_id.min(16).max(1) as f32) + 2.0
            );
        }
    }

    // Second materialize should not add pending (all cached)
    cache.materialize(&backend, &font, &shaped).unwrap();
    assert_eq!(
        cache.pending_len(),
        0,
        "no new pending after re-materialize"
    );

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
    let mut backend = MockBackend;
    let font = backend.load_font("Test", 16.0, 1.0).unwrap();
    let shaped = backend.shape_line(&font, "a").unwrap();

    cache.materialize(&backend, &font, &shaped).unwrap();
    cache.flush(&mut atlas, &queue).unwrap();

    let fh = font.handle();
    let gid = shaped.glyphs[0].glyph_id;

    let first = cache.get(fh, gid, 0).unwrap();
    let first_id = first.alloc.alloc_id;

    let second = cache.get(fh, gid, 0).unwrap();
    assert_eq!(
        first_id, second.alloc.alloc_id,
        "same key should return same alloc_id"
    );
}
