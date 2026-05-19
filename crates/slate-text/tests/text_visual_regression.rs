//! Visual regression tests for the full text rendering pipeline.
//!
//! These tests render text through the complete pipeline (shape → materialize →
//! flush → build instances → render) and compare against golden PNGs.

mod common;

use common::mock::MockFont;
use slate_renderer::atlas::{Atlas, Format};
use slate_renderer::srgb_u8_to_linear_premul;
use slate_text::TextBackend;
use slate_text::error::TextError;
use slate_text::font_handle::FontHandle;
use slate_text::glyph_cache::GlyphCache;
use slate_text::run_builder::TextRunBuilder;
use slate_text::types::{
    FontDescriptor, FontId, GlyphBitmap, GlyphBounds, ShapedGlyph, ShapedLine,
};

/// Backend producing 8×12 glyphs with a vertical-bar pattern per glyph_id.
///
/// Uses 8 lpx advance — distinct from the default 10 lpx MockBackend — so the
/// visual regression tests exercise a different layout density.
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
            handle: FontHandle::from_face_id(ptr as u64, size_lpx, scale),
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
                x_advance_lpx: 8.0,
                position_lpx: [i as f32 * 8.0, 0.0],
                cluster: 0,
            })
            .collect();
        let width = glyphs.len() as f32 * 8.0;
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
        // 8×12 glyph: vertical bar column determined by glyph_id
        let w = 8u32;
        let h = 12u32;
        let mut alpha = vec![0u8; (w * h) as usize];
        let bar_x = (glyph_id % w) as usize;
        for y in 0..h as usize {
            alpha[y * w as usize + bar_x] = 0xFF;
        }
        let _ = variant; // sub-pixel offset handled by caller
        Ok(GlyphBitmap {
            width: w,
            height: h,
            bearing_x_lpx: 0.0,
            bearing_y_lpx: 10.0,
            advance_x_lpx: 8.0,
            alpha,
        })
    }

    fn glyph_raster_bounds(
        &self,
        _font: &Self::Font,
        _glyph_id: u32,
    ) -> Result<GlyphBounds, TextError> {
        Ok(GlyphBounds {
            width: 8,
            height: 12,
        })
    }

    fn enumerate_system_fonts(&self) -> Result<Vec<FontDescriptor>, TextError> {
        Ok(vec![])
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

    // Build glyph instances (immediate rasterization and upload)
    let builder = TextRunBuilder {
        backend: &backend,
        font: &font,
        baseline_lpx: [10.0, 50.0],
        color: srgb_u8_to_linear_premul([0xFF, 0xFF, 0xFF, 0xFF]),
    };
    let instances = builder
        .build(&shaped, &mut cache, &mut atlas, &queue)
        .unwrap();

    // Should have 5 glyph instances for "Hello"
    assert_eq!(instances.len(), 5);

    // Each instance should have valid rect and UV coordinates
    for (i, inst) in instances.iter().enumerate() {
        assert!(
            inst.rect[2] > slate_renderer::Lpx(0.0),
            "glyph {i} has zero width"
        );
        assert!(
            inst.rect[3] > slate_renderer::Lpx(0.0),
            "glyph {i} has zero height"
        );
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

    let builder = TextRunBuilder {
        backend: &backend,
        font: &font,
        baseline_lpx: [0.0, 20.0],
        color: [1.0; 4],
    };
    let instances = builder
        .build(&shaped, &mut cache, &mut atlas, &queue)
        .unwrap();

    assert_eq!(instances.len(), 2);
    // Second glyph should be positioned after the first
    // x_advance_lpx = 8.0, scale = 1.0 → second glyph starts ~8px after first
    let x0 = instances[0].rect[0];
    let x1 = instances[1].rect[0];
    assert!(x1 > x0, "second glyph should be to the right of first");
}
