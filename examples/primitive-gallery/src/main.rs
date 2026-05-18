mod scene_builder;

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;

use slate_platform::{DefaultPlatform, Event, Platform, Window, WindowOptions};
use slate_renderer::{Renderer, Scene, srgb_u8_to_linear_premul};

#[cfg(target_os = "windows")]
use slate_text::DirectWriteBackend as TextBackendImpl;

#[cfg(target_os = "macos")]
use slate_text::CoreTextBackend as TextBackendImpl;

use slate_text::{
    Font, GlyphCache, LineLayoutCache, TEST_FONT, TextAlignment, TextBackend, TextRunBuilder,
    compute_alignment_offset,
};

use scene_builder::{IMG_SIZE, build_scene, generate_checkerboard};

fn main() {
    env_logger::init();
    let start = Instant::now();

    let platform = DefaultPlatform::new();
    let window: Arc<_> = platform.create_window(WindowOptions {
        title: "Slate \u{00b7} Primitive Gallery".into(),
        size: (1280, 800),
        min_size: Some((320, 240)),
        resizable: true,
        ..Default::default()
    });

    let renderer: RefCell<Option<Renderer>> = RefCell::new(None);
    let scene = RefCell::new(Scene::new());
    let image_uv: RefCell<[f32; 4]> = RefCell::new([0.0; 4]);
    let frame_count: RefCell<u64> = RefCell::new(0);
    let last_fps_time: RefCell<Instant> = RefCell::new(Instant::now());
    let current_fps: RefCell<f32> = RefCell::new(0.0);

    // Text system state
    let text_backend: RefCell<Option<TextBackendImpl>> = RefCell::new(None);
    let text_font: RefCell<Option<<TextBackendImpl as TextBackend>::Font>> = RefCell::new(None);
    let glyph_cache: RefCell<GlyphCache> = RefCell::new(GlyphCache::new());
    let line_cache: RefCell<LineLayoutCache> = RefCell::new(LineLayoutCache::new());

    let platform_ref = &platform;
    let window_ref = window.clone();

    platform.run(move |event| match event {
        Event::Resumed => {
            let mut r = match pollster::block_on(Renderer::new(window_ref.clone())) {
                Ok(r) => r,
                Err(e) => {
                    log::error!("renderer init failed: {e}");
                    platform_ref.quit();
                    return;
                }
            };

            // Seed image atlas: procedural 256x256 checkerboard
            let checker = generate_checkerboard();
            let img_alloc = r.image_atlas_mut().allocate(IMG_SIZE, IMG_SIZE).unwrap();
            r.image_atlas_mut().pin(img_alloc.alloc_id);
            r.upload_to_image_atlas(img_alloc.alloc_id, &checker);
            *image_uv.borrow_mut() = img_alloc.uv_rect;

            // Initialize text backend and load font
            let scale = window_ref.scale_factor() as f32;
            let mut backend = TextBackendImpl::new().expect("text backend init");
            let font = backend
                .load_font_from_bytes(TEST_FONT, 16.0, scale)
                .expect("font load");

            log::info!(
                "renderer ready, text system initialized in {:.1}ms",
                start.elapsed().as_secs_f32() * 1000.0
            );

            *text_font.borrow_mut() = Some(font);
            *text_backend.borrow_mut() = Some(backend);
            *renderer.borrow_mut() = Some(r);
        }

        Event::WindowResized { physical_size, .. } => {
            if let Some(r) = renderer.borrow_mut().as_mut() {
                r.resize(physical_size, window_ref.logical_size());
            }
        }

        Event::WindowRedrawRequested { .. } => {
            if renderer.borrow().is_none() {
                return;
            }

            // Scene wire format is in logical pixels; the renderer's viewport
            // maps lpx → NDC and the surface is configured in physical pixels.
            let (w_lpx, h_lpx) = window_ref.logical_size();
            let scale = window_ref.scale_factor() as f32;
            let fps = *current_fps.borrow();
            let uv = *image_uv.borrow();

            // Build text glyphs using native text pipeline
            let text_glyphs = build_text_glyphs(
                &text_backend,
                &text_font,
                &glyph_cache,
                &line_cache,
                &renderer,
                fps,
                scale,
                w_lpx as f32,
            );

            // Finish line layout cache frame (two-frame rolling)
            line_cache.borrow().finish_frame();

            let mut s = scene.borrow_mut();
            build_scene(&mut s, w_lpx as f32, h_lpx as f32, uv, &text_glyphs);

            let result = renderer.borrow_mut().as_mut().unwrap().render_scene(&mut s);
            if let Err(e) = result {
                log::warn!("render skipped: {e:?}");
            }
            drop(s);

            // FPS calculation (0.5s window)
            let mut fc = frame_count.borrow_mut();
            *fc += 1;
            let elapsed = last_fps_time.borrow().elapsed().as_secs_f32();
            if elapsed >= 0.5 {
                *current_fps.borrow_mut() = *fc as f32 / elapsed;
                *fc = 0;
                *last_fps_time.borrow_mut() = Instant::now();
            }
        }

        Event::WindowCloseRequested { .. } => platform_ref.quit(),
        Event::Exiting => log::info!("bye"),
        _ => {}
    });
}

use slate_renderer::GlyphInstance;

#[allow(clippy::too_many_arguments)]
fn build_text_glyphs(
    backend_cell: &RefCell<Option<TextBackendImpl>>,
    font_cell: &RefCell<Option<<TextBackendImpl as TextBackend>::Font>>,
    glyph_cache_cell: &RefCell<GlyphCache>,
    line_cache_cell: &RefCell<LineLayoutCache>,
    renderer_cell: &RefCell<Option<Renderer>>,
    fps: f32,
    scale: f32,
    w: f32,
) -> Vec<GlyphInstance> {
    let backend = backend_cell.borrow();
    let font = font_cell.borrow();
    let (Some(backend), Some(font)) = (backend.as_ref(), font.as_ref()) else {
        return Vec::new();
    };

    let mut glyph_cache = glyph_cache_cell.borrow_mut();
    let line_cache = line_cache_cell.borrow();
    let mut renderer = renderer_cell.borrow_mut();
    let renderer = renderer.as_mut().unwrap();

    // Get atlas and queue for immediate glyph upload
    let (atlas, queue) = renderer.glyph_atlas_and_queue();

    // Get font handle for cache keying
    let font_handle = font.handle();

    // Shape text lines (cached via LineLayoutCache)
    let hello_text = "Hello, world!";
    let fps_text = format!("{fps:.1} fps");

    let hello_shaped = line_cache.get_or_shape(hello_text, font_handle, || {
        backend.shape_line(font, hello_text).unwrap()
    });
    let fps_shaped = line_cache.get_or_shape(&fps_text, font_handle, || {
        backend.shape_line(font, &fps_text).unwrap()
    });

    // Build glyph instances (immediate rasterization and upload)
    let mut glyphs = Vec::new();

    // Hello world - white text at top-right area
    let hello_builder = TextRunBuilder {
        backend,
        font,
        baseline_lpx: [w / scale - hello_shaped.width_lpx - 20.0, 60.0],
        color: srgb_u8_to_linear_premul([0xFF, 0xFF, 0xFF, 0xFF]),
    };
    if let Ok(instances) = hello_builder.build(&hello_shaped, &mut glyph_cache, atlas, queue) {
        glyphs.extend(instances);
    }

    // FPS counter - yellow text at top-right corner
    let fps_builder = TextRunBuilder {
        backend,
        font,
        baseline_lpx: [w / scale - fps_shaped.width_lpx - 10.0, 25.0],
        color: srgb_u8_to_linear_premul([0xFF, 0xFF, 0x00, 0xFF]),
    };
    if let Ok(instances) = fps_builder.build(&fps_shaped, &mut glyph_cache, atlas, queue) {
        glyphs.extend(instances);
    }

    // Multi-line paragraph demo with word wrap. Keep position responsive: stick
    // to the right side of the window with a sane minimum so the paragraph
    // tracks shrink/grow instead of clipping at hardcoded x=650.
    let paragraph_text = "This is a multi-line paragraph with automatic word wrapping. \
        The greedy algorithm breaks text at spaces to fit within the maximum width.";
    let max_width = 280.0;
    let w_lpx = w / scale;
    let paragraph_x = (w_lpx - max_width - 20.0).max(20.0);
    let paragraph_y = 50.0;

    if let Ok(paragraph_lines) = backend.shape_paragraph(font, paragraph_text, max_width) {
        for line in &paragraph_lines {
            let x_offset = compute_alignment_offset(line.width_lpx, max_width, TextAlignment::Left);
            let builder = TextRunBuilder {
                backend,
                font,
                baseline_lpx: [paragraph_x + x_offset, paragraph_y + line.y_offset_lpx],
                color: srgb_u8_to_linear_premul([0xCC, 0xCC, 0xCC, 0xFF]),
            };
            if let Ok(instances) = builder.build(line, &mut glyph_cache, atlas, queue) {
                glyphs.extend(instances);
            }
        }
    }

    // Extended Latin demo (font fallback showcase - no emoji, deferred to 2c)
    let extended_text = "Extended: Grüß Gott! Ça va? Ñoño";
    let extended_shaped = line_cache.get_or_shape(extended_text, font_handle, || {
        backend.shape_line(font, extended_text).unwrap()
    });
    let extended_builder = TextRunBuilder {
        backend,
        font,
        baseline_lpx: [paragraph_x, paragraph_y + 90.0],
        color: srgb_u8_to_linear_premul([0x88, 0xCC, 0xFF, 0xFF]),
    };
    if let Ok(instances) = extended_builder.build(&extended_shaped, &mut glyph_cache, atlas, queue)
    {
        glyphs.extend(instances);
    }

    // Light-on-dark demo (tests font smoothing dilation on macOS)
    // Note: The dark rect is rendered in scene_builder; we just add white text here
    let light_text = "Light on dark";
    let light_shaped = line_cache.get_or_shape(light_text, font_handle, || {
        backend.shape_line(font, light_text).unwrap()
    });
    let light_builder = TextRunBuilder {
        backend,
        font,
        baseline_lpx: [paragraph_x + 10.0, paragraph_y + 140.0],
        color: srgb_u8_to_linear_premul([0xFF, 0xFF, 0xFF, 0xFF]),
    };
    if let Ok(instances) = light_builder.build(&light_shaped, &mut glyph_cache, atlas, queue) {
        glyphs.extend(instances);
    }

    glyphs
}
