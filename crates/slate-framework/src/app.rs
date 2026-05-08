//! Application container and frame loop.
//!
//! `App` owns all framework resources and provides `run()` to enter
//! the platform event loop with a View.

use std::cell::RefCell;
use std::sync::Arc;

use slate_platform::{
    wake_run_loop, DefaultPlatform, DefaultWindow, Event, Platform, Window, WindowOptions,
};
use slate_renderer::{Renderer, Scene};

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::executor::{Executor, RedrawRequester};
use crate::hit_test::HitTestList;
use crate::layout::{compute_layout, resolve_bounds, LayoutTree};
use crate::text_system::TextSystem;
use crate::types::{AccessibilityNode, Size};
use crate::view::View;

/// Application container.
///
/// Owns all framework resources: platform, window, renderer, executor,
/// layout tree, hit-test list, accessibility tree, and text system.
///
/// # Example
///
/// ```ignore
/// App::new(WindowOptions {
///     title: "My App".into(),
///     size: (800, 600),
///     ..Default::default()
/// })
/// .run(|| MyView::new());
/// ```
pub struct App {
    platform: DefaultPlatform,
    window: Arc<DefaultWindow>,
}

impl App {
    /// Create a new application with the given window options.
    ///
    /// The renderer and text system are initialized lazily on `Event::Resumed`.
    pub fn new(options: WindowOptions) -> Self {
        let platform = DefaultPlatform::new();
        let window = platform.create_window(options);

        Self { platform, window }
    }

    /// Run the application with the given view factory.
    ///
    /// `view_fn` is `FnMut` because `Event::Resumed` can fire multiple times
    /// (e.g., after suspend on mobile platforms).
    ///
    /// This method enters the platform event loop and does not return until
    /// the application exits.
    pub fn run<V: View>(self, mut view_fn: impl FnMut() -> V + 'static) {
        let App { platform, window } = self;

        // Deferred initialization (set up in Event::Resumed)
        let renderer: RefCell<Option<Renderer>> = RefCell::new(None);
        let text_system: RefCell<Option<TextSystem>> = RefCell::new(None);
        let view: RefCell<Option<V>> = RefCell::new(None);

        // Per-frame state (always allocated)
        let redraw_requester = RedrawRequester::new(wake_run_loop);
        let executor = Executor::new(redraw_requester);
        let layout_tree: RefCell<LayoutTree> = RefCell::new(LayoutTree::new());
        let hit_test_list: RefCell<HitTestList> = RefCell::new(HitTestList::new());
        let a11y_nodes: RefCell<Vec<AccessibilityNode>> = RefCell::new(Vec::new());
        let scene: RefCell<Scene> = RefCell::new(Scene::new());

        let platform_ref = &platform;
        let window_ref = window.clone();
        let executor_ref = &executor;

        platform.run(move |event| match event {
            Event::Resumed => {
                // Initialize renderer
                let r = match pollster::block_on(Renderer::new(window_ref.clone())) {
                    Ok(r) => r,
                    Err(e) => {
                        log::error!("renderer init failed: {e}");
                        platform_ref.quit();
                        return;
                    }
                };

                // Initialize text system
                let ts = match TextSystem::new() {
                    Ok(ts) => ts,
                    Err(e) => {
                        log::error!("text system init failed: {e}");
                        platform_ref.quit();
                        return;
                    }
                };

                log::info!("renderer and text system ready");

                *renderer.borrow_mut() = Some(r);
                *text_system.borrow_mut() = Some(ts);
                *view.borrow_mut() = Some(view_fn());

                // Request initial redraw
                window_ref.request_redraw();
            }

            Event::WindowResized { size, .. } => {
                if let Some(r) = renderer.borrow_mut().as_mut() {
                    r.resize(size);
                }
            }

            Event::WindowRedrawRequested { .. } => {
                // Skip if not initialized
                if renderer.borrow().is_none() {
                    return;
                }

                let (w, h) = window_ref.size();
                let scale_factor = window_ref.scale_factor();

                // 1. Build element tree
                let mut root = {
                    let mut v = view.borrow_mut();
                    let v = v.as_mut().expect("view not initialized");
                    v.render()
                };

                // 2. Layout pass
                let root_id = {
                    let mut tree = layout_tree.borrow_mut();
                    tree.clear();

                    let mut ts = text_system.borrow_mut();
                    let ts = ts.as_mut().expect("text system not initialized");

                    let mut cx = LayoutCtx::new(
                        tree.inner_mut(),
                        ts,
                        &executor_ref.foreground,
                        scale_factor,
                    );

                    compute_layout(
                        &mut root,
                        &mut cx,
                        Size::new(w as f32, h as f32),
                    )
                };

                let Some(root_id) = root_id else {
                    log::warn!("layout computation failed");
                    return;
                };

                // 3. Resolve root bounds
                let root_bounds = {
                    let tree = layout_tree.borrow();
                    resolve_bounds(tree.inner(), root_id)
                };

                let Some(root_bounds) = root_bounds else {
                    log::warn!("bounds resolution failed");
                    return;
                };

                // 4. Prepaint pass
                {
                    let tree = layout_tree.borrow();
                    let mut hit = hit_test_list.borrow_mut();
                    let mut a11y = a11y_nodes.borrow_mut();
                    let mut ts = text_system.borrow_mut();
                    let ts = ts.as_mut().expect("text system not initialized");

                    hit.clear();
                    a11y.clear();

                    let mut cx = PrepaintCtx::new(
                        tree.inner(),
                        &mut hit,
                        &mut a11y,
                        ts,
                        &executor_ref.foreground,
                        scale_factor,
                    );

                    root.prepaint(root_bounds, &mut cx);
                }

                // 5. Paint pass (split borrow on renderer for atlas/queue)
                {
                    let tree = layout_tree.borrow();
                    let mut s = scene.borrow_mut();
                    let mut r = renderer.borrow_mut();
                    let r = r.as_mut().expect("renderer not initialized");
                    let mut ts = text_system.borrow_mut();
                    let ts = ts.as_mut().expect("text system not initialized");

                    s.clear();

                    let (atlas, queue) = r.glyph_atlas_and_queue();
                    let mut cx = PaintCtx::new(
                        tree.inner(),
                        &mut s,
                        ts,
                        atlas,
                        queue,
                        &executor_ref.foreground,
                        scale_factor,
                    );

                    root.paint(root_bounds, &mut cx);
                }

                // 6. Render
                {
                    let mut s = scene.borrow_mut();
                    let mut r = renderer.borrow_mut();
                    let r = r.as_mut().expect("renderer not initialized");

                    if let Err(e) = r.render_scene(&mut s) {
                        log::warn!("render skipped: {e:?}");
                    }
                }

                // 7. Poll async executor
                executor_ref.foreground.poll();
            }

            Event::WindowCloseRequested { .. } => {
                platform_ref.quit();
            }

            Event::Exiting => {
                log::info!("exiting");
            }

            _ => {}
        });
    }
}
