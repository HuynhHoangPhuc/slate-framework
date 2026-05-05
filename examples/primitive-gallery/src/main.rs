mod scene_builder;

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;

use slate_platform::{DefaultPlatform, Event, Platform, Window, WindowOptions};
use slate_renderer::{Renderer, Scene, allocate_glyph};

use scene_builder::{
    DIGIT_BITS, GLYPH_H, GLYPH_W, IMG_SIZE, build_scene, generate_checkerboard, padded_glyph_pixels,
};

fn main() {
    env_logger::init();
    let start = Instant::now();

    let platform = DefaultPlatform::new();
    let window: Arc<_> = platform.create_window(WindowOptions {
        title: "Slate \u{00b7} Primitive Gallery".into(),
        size: (1280, 800),
        min_size: Some((320, 240)),
        resizable: true,
    });

    let renderer: RefCell<Option<Renderer>> = RefCell::new(None);
    let scene = RefCell::new(Scene::new());
    let digit_uvs: RefCell<Vec<[f32; 4]>> = RefCell::new(Vec::new());
    let image_uv: RefCell<[f32; 4]> = RefCell::new([0.0; 4]);
    let frame_count: RefCell<u64> = RefCell::new(0);
    let last_fps_time: RefCell<Instant> = RefCell::new(Instant::now());
    let current_fps: RefCell<f32> = RefCell::new(0.0);

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

            // Seed glyph atlas: 11 dot-matrix digit bitmaps (0-9, '.')
            let mut uvs = Vec::with_capacity(11);
            for &bits in &DIGIT_BITS {
                let (alloc_id, uv_rect) =
                    allocate_glyph(r.glyph_atlas_mut(), GLYPH_W, GLYPH_H).unwrap();
                r.glyph_atlas_mut().pin(alloc_id);
                r.upload_to_glyph_atlas(alloc_id, &padded_glyph_pixels(bits));
                uvs.push(uv_rect);
            }
            *digit_uvs.borrow_mut() = uvs;

            log::info!(
                "renderer ready, atlas seeded in {:.1}ms",
                start.elapsed().as_secs_f32() * 1000.0
            );
            *renderer.borrow_mut() = Some(r);
            window_ref.request_redraw();
        }

        Event::WindowResized { size, .. } => {
            if let Some(r) = renderer.borrow_mut().as_mut() {
                r.resize(size);
            }
        }

        Event::WindowRedrawRequested { .. } => {
            if renderer.borrow().is_none() {
                return;
            }

            let (w, h) = window_ref.size();
            let scale = window_ref.scale_factor() as f32;
            let fps = *current_fps.borrow();

            let uvs = digit_uvs.borrow();
            let uv = *image_uv.borrow();

            let mut s = scene.borrow_mut();
            build_scene(&mut s, w as f32, h as f32, scale, uv, &uvs, fps);
            drop(uvs);

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
