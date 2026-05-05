//! hello-rect — Phase 7 anchor demo.
//!
//! Opens a native window, initialises the renderer, and draws a single
//! antialiased rounded rectangle via the Scene-driven instanced rect pipeline.
//!
//! Run: `cargo run -p hello-rect`

use std::cell::RefCell;
use std::sync::Arc;

use slate_platform::{DefaultPlatform, Event, Platform, Window, WindowOptions};
use slate_renderer::{RectInstance, Renderer, Scene, srgb_u8_to_linear_premul};

fn main() {
    env_logger::init();

    let platform = DefaultPlatform::new();
    let window: Arc<_> = platform.create_window(WindowOptions {
        title: "Slate · hello-rect".into(),
        size: (800, 600),
        min_size: Some((320, 240)),
        resizable: true,
    });

    let renderer: RefCell<Option<Renderer>> = RefCell::new(None);
    let scene: RefCell<Scene> = RefCell::new(Scene::new());

    let platform_ref = &platform;
    let window_ref = window.clone();

    platform.run(move |event| match event {
        Event::Resumed => {
            let r = match pollster::block_on(Renderer::new(window_ref.clone())) {
                Ok(r) => r,
                Err(e) => {
                    log::error!("renderer init failed: {e}");
                    platform_ref.quit();
                    return;
                }
            };
            log::info!("renderer ready");
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

            let mut s = scene.borrow_mut();
            s.clear();
            s.push_rect(RectInstance {
                rect: [
                    w as f32 / 2.0 - 200.0 * scale,
                    h as f32 / 2.0 - 100.0 * scale,
                    400.0 * scale,
                    200.0 * scale,
                ],
                color: srgb_u8_to_linear_premul([0x66, 0xcc, 0xff, 0xff]),
                corner_radius: 24.0 * scale,
                _pad: [0.0; 3],
            });

            let result = renderer.borrow_mut().as_mut().unwrap().render_scene(&mut s);
            if let Err(e) = result {
                log::warn!("render skipped: {e:?}");
            }
        }

        Event::WindowCloseRequested { .. } => platform_ref.quit(),

        Event::Exiting => log::info!("bye"),

        _ => {}
    });
}
