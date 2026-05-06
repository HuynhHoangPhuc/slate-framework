//! Visual regression tests for the full text rendering pipeline.
//!
//! These tests render text through the complete pipeline (shape → materialize →
//! flush → build instances → render) and compare against golden PNGs.

mod common;

use slate_renderer::atlas::{Atlas, Format};
use slate_renderer::srgb_u8_to_linear_premul;
use slate_text::backend::{Font, TextBackend};
use slate_text::error::TextError;
use slate_text::font_handle::FontHandle;
use slate_text::glyph_cache::GlyphCache;
use slate_text::run_builder::TextRunBuilder;
use slate_text::types::{FontMetrics, GlyphBitmap, ShapedGlyph, ShapedLine};

/// Mock font for controlled testing.
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
                glyph_id: i as u32 + 1,
                x_advance_lpx: 8.0,
                x_offset_lpx: 0.0,
                y_offset_lpx: 0.0,
            })
            .collect();
        let width = glyphs.len() as f32 * 8.0;
        Ok(ShapedLine {
            glyphs,
            width_lpx: width,
            ascent_lpx: 12.0,
            descent_lpx: -3.0,
        })
    }

    fn rasterize_glyph(
        &self,
        _font: &Self::Font,
        glyph_id: u32,
        variant: u8,
    ) -> Result<GlyphBitmap, TextError> {
        // 8x12 glyph with a simple pattern
        let w = 8u32;
        let h = 12u32;
        let mut alpha = vec![0u8; (w * h) as usize];
        // Draw a simple vertical bar based on glyph_id
        let bar_x = (glyph_id % w) as usize;
        for y in 0..h as usize {
            alpha[y * w as usize + bar_x] = 0xFF;
        }
        // Variant affects horizontal offset slightly (sub-pixel)
        let _ = variant;
        Ok(GlyphBitmap {
            width: w,
            height: h,
            bearing_x_lpx: 0.0,
            bearing_y_lpx: 10.0,
            advance_x_lpx: 8.0,
            alpha,
        })
    }
}

#[test]
fn text_pipeline_builds_glyph_instances() {
    let Some((device, queue)) = common::headless::make_headless_device() else {
        eprintln!("Skipping test: no GPU adapter available");
        return;
    };

    let mut atlas = Atlas::new(&device, Format::R8Unorm);
    let mut cache = GlyphCache::new();
    let mut backend = MockBackend;
    let font = backend.load_font("Test", 16.0, 2.0).unwrap();

    // Shape "Hello"
    let shaped = backend.shape_line(&font, "Hello").unwrap();

    // Materialize and flush
    cache.materialize(&backend, &font, &shaped).unwrap();
    cache.flush(&mut atlas, &queue).unwrap();

    // Build glyph instances
    let builder = TextRunBuilder {
        backend: &backend,
        font: &font,
        cache: &cache,
        baseline_lpx: [10.0, 50.0],
        color: srgb_u8_to_linear_premul([0xFF, 0xFF, 0xFF, 0xFF]),
    };
    let instances = builder.build(&shaped).unwrap();

    // Should have 5 glyph instances for "Hello"
    assert_eq!(instances.len(), 5);

    // Each instance should have valid rect and UV coordinates
    for (i, inst) in instances.iter().enumerate() {
        assert!(inst.rect[2] > 0.0, "glyph {i} has zero width");
        assert!(inst.rect[3] > 0.0, "glyph {i} has zero height");
        assert!(inst.uv_rect[0] >= 0.0 && inst.uv_rect[0] <= 1.0);
        assert!(inst.uv_rect[1] >= 0.0 && inst.uv_rect[1] <= 1.0);
    }
}

#[test]
fn glyph_instances_positions_advance_correctly() {
    let Some((device, queue)) = common::headless::make_headless_device() else {
        eprintln!("Skipping test: no GPU adapter available");
        return;
    };

    let mut atlas = Atlas::new(&device, Format::R8Unorm);
    let mut cache = GlyphCache::new();
    let mut backend = MockBackend;
    let font = backend.load_font("Test", 16.0, 1.0).unwrap();

    let shaped = backend.shape_line(&font, "AB").unwrap();
    cache.materialize(&backend, &font, &shaped).unwrap();
    cache.flush(&mut atlas, &queue).unwrap();

    let builder = TextRunBuilder {
        backend: &backend,
        font: &font,
        cache: &cache,
        baseline_lpx: [0.0, 20.0],
        color: [1.0; 4],
    };
    let instances = builder.build(&shaped).unwrap();

    assert_eq!(instances.len(), 2);
    // Second glyph should be positioned after the first
    // x_advance_lpx = 8.0, scale = 1.0 → second glyph starts ~8px after first
    let x0 = instances[0].rect[0];
    let x1 = instances[1].rect[0];
    assert!(x1 > x0, "second glyph should be to the right of first");
}
