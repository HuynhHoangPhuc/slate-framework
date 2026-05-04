//! hello-rect — Phase 0 anchor demo.
//!
//! Opens a native window, initialises a wgpu renderer, and draws a single
//! antialiased rounded rectangle via the SDF rect pipeline.
//!
//! Run: `cargo run --example hello-rect`

use std::cell::RefCell;
use std::sync::Arc;

use slate_platform::{DefaultPlatform, Event, Platform, Window, WindowOptions};
use slate_renderer::{RectPipeline, RectUniform, Renderer};

fn main() {
    env_logger::init();

    let platform = DefaultPlatform::new();
    let window: Arc<_> = platform.create_window(WindowOptions {
        title: "Slate · hello-rect".into(),
        size: (800, 600),
        min_size: Some((320, 240)),
        resizable: true,
    });

    // Late-bound GPU state. Both are None until Event::Resumed fires (H2).
    // RefCell for interior mutability across FnMut(Event) calls (C5).
    let renderer: RefCell<Option<Renderer>> = RefCell::new(None);
    let pipeline: RefCell<Option<RectPipeline>> = RefCell::new(None);

    // Borrow platform by reference so we can call quit() from inside the
    // closure without moving it (Platform::run takes &self, not self) (C3).
    let platform_ref = &platform;
    let window_ref = window.clone();

    platform.run(move |event| {
        match event {
            Event::Resumed => {
                // Renderer must be created here — after the OS run loop is alive.
                // On macOS: NSApplication must be running before CAMetalLayer attaches.
                let r = match pollster::block_on(Renderer::new(window_ref.clone())) {
                    Ok(r) => r,
                    Err(e) => {
                        log::error!("renderer init failed: {e}");
                        platform_ref.quit();
                        return;
                    }
                };
                let p = RectPipeline::new(r.device(), r.surface_format());
                log::info!("renderer ready");
                *renderer.borrow_mut() = Some(r);
                *pipeline.borrow_mut() = Some(p);
            }

            Event::WindowResized { size, .. } => {
                // size is already in physical pixels (Window::size() contract).
                if let Some(r) = renderer.borrow_mut().as_mut() {
                    r.resize(size);
                }
            }

            Event::WindowRedrawRequested { .. } => {
                // H6: silently skip until late init is complete.
                if renderer.borrow().is_none() || pipeline.borrow().is_none() {
                    return;
                }

                // window.size() returns physical pixels — use directly, no scale multiply.
                let (w, h) = window_ref.size();
                let scale = window_ref.scale_factor() as f32;

                // Upload uniform (immutable borrows released before &mut renderer).
                {
                    let r_ref = renderer.borrow();
                    let p_ref = pipeline.borrow();
                    let r = r_ref.as_ref().unwrap();
                    let p = p_ref.as_ref().unwrap();
                    p.upload(
                        r.queue(),
                        RectUniform {
                            viewport_size: [w as f32, h as f32],
                            center: [w as f32 / 2.0, h as f32 / 2.0],
                            // Logical 200×100 points → physical pixels.
                            half_size: [200.0 * scale, 100.0 * scale],
                            // Logical 24 pt corner radius → physical pixels.
                            corner_radius: 24.0 * scale,
                            // #66ccff in linear sRGB ≈ [0.40, 0.80, 1.00, 1.0].
                            color: [0.4, 0.8, 1.0, 1.0],
                            _pad: 0.0,
                        },
                    );
                }

                // RectPipeline::record(&self, &mut RenderPass<'_>) has independent
                // lifetimes (wgpu 29 set_pipeline/set_bind_group don't tie &self to 'pass),
                // so borrowing pipeline inside the closure is safe.
                // No ? inside FnMut(Event) — log and continue.
                let result = renderer.borrow_mut().as_mut().unwrap().render_with(|rpass| {
                    pipeline.borrow().as_ref().unwrap().record(rpass);
                });
                if let Err(e) = result {
                    log::warn!("render skipped: {e:?}");
                }
            }

            Event::WindowCloseRequested { .. } => platform_ref.quit(),

            Event::Exiting => log::info!("bye"),

            _ => {}
        }
    });
}
