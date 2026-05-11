//! Application container and frame loop.
//!
//! `App` owns all framework resources and provides `run()` to enter
//! the platform event loop with a View.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use slate_platform::{
    DefaultPlatform, DefaultWindow, Event, Platform, Window, WindowOptions,
    clear_render_callback, set_render_callback, wake_run_loop,
};
#[cfg(target_os = "windows")]
use slate_platform::{clear_pump_executor_callback, set_pump_executor_callback};
use slate_renderer::Renderer;

use crate::app_state::{
    AppState, bubble_mouse_handler, bubble_pointer_handler, bubble_scroll_handler, button_to_bit,
    fire_hover_transitions, run_redraw, run_resize_sync,
};
use crate::event::{MouseEvent, PointerEvent, PointerEventKind, ScrollEvent};
use crate::executor::{BackgroundExecutor, Executor, RedrawRequester};
use crate::text_system::TextSystem;
use crate::types::Point;
use crate::view::View;
use slate_platform::MouseButton;

/// Application context passed to the view factory.
///
/// Provides access to the reactive runtime and background executor for constructing
/// signals and spawning background tasks.
///
/// Note: `ForegroundExecutor` is intentionally not exposed here because it's `!Send`
/// and bound to the UI thread. UI-thread tasks should use the foreground executor
/// available in element context methods.
///
/// # Example
///
/// ```ignore
/// App::new(options).run(|cx| {
///     let count = Signal::new(cx.runtime(), 0u32);
///     cx.background_executor().spawn(async move { /* ... */ }).detach();
///     MyView { count }
/// });
/// ```
#[derive(Clone)]
pub struct AppContext {
    runtime: Arc<slate_reactive::Runtime>,
    background_executor: BackgroundExecutor,
}

impl AppContext {
    /// Get the reactive runtime for creating signals.
    pub fn runtime(&self) -> Arc<slate_reactive::Runtime> {
        self.runtime.clone()
    }

    /// Get the background executor for spawning async tasks.
    pub fn background_executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }
}

// Debug-mode borrow-order discipline (see ADR-001)
#[cfg(debug_assertions)]
thread_local! {
    static BORROW_ORDER: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(debug_assertions)]
fn reset_borrow_order() {
    BORROW_ORDER.with(|c| c.set(0));
}

#[cfg(debug_assertions)]
#[allow(dead_code)]
fn check_borrow_order(slot: u8) {
    BORROW_ORDER.with(|c| {
        let last = c.get();
        debug_assert!(
            slot > last,
            "RefCell borrow-order violation: tried slot {} after slot {}; see ADR-001",
            slot,
            last
        );
        c.set(slot);
    });
}

#[cfg(not(debug_assertions))]
fn reset_borrow_order() {}

#[cfg(not(debug_assertions))]
#[allow(dead_code)]
fn check_borrow_order(_slot: u8) {}

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
/// .run(|cx| {
///     let count = Signal::new(cx.runtime(), 0);
///     MyView::new(count)
/// });
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
    /// `view_fn` receives an [`AppContext`] with access to the reactive runtime
    /// and background executor for constructing signals and spawning tasks.
    ///
    /// `view_fn` is `FnMut` because `Event::Resumed` can fire multiple times
    /// (e.g., after suspend on mobile platforms).
    ///
    /// This method enters the platform event loop and does not return until
    /// the application exits.
    pub fn run<V: View>(self, mut view_fn: impl FnMut(&AppContext) -> V + 'static) {
        let App { platform, window } = self;

        // Create executor and redraw requester
        let redraw_requester = RedrawRequester::new(wake_run_loop);
        let executor = Executor::new(redraw_requester.clone());

        // Create reactive runtime with redraw wiring
        let runtime = slate_reactive::Runtime::new();
        runtime.install_redraw({
            let req = redraw_requester.clone();
            Arc::new(move || req.request())
        });

        // Create shared AppState
        let state: Rc<AppState<V>> = Rc::new(AppState::new(
            window.clone(),
            executor,
            redraw_requester,
            runtime.clone(),
        ));

        // Register resize callback (Phase 1 plumbing — not invoked until Phase 2/3)
        {
            let s = Rc::clone(&state);
            set_render_callback(Box::new(move |window_id, size| {
                run_resize_sync(&s, window_id, size);
            }));
        }

        // Register pump executor callback for WM_TIMER during size-move modal loop (Windows only)
        #[cfg(target_os = "windows")]
        {
            let s = Rc::clone(&state);
            set_pump_executor_callback(Box::new(move || {
                s.executor.foreground.poll();
            }));
        }

        // AppContext for view factory
        let cx = AppContext {
            runtime: runtime.clone(),
            background_executor: state.executor.background.clone(),
        };

        let platform_ref = &platform;
        let window_ref = window.clone();
        let state_ref = Rc::clone(&state);

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

                *state_ref.renderer.borrow_mut() = Some(r);
                *state_ref.text_system.borrow_mut() = Some(ts);
                *state_ref.view.borrow_mut() = Some(view_fn(&cx));

                // Request initial redraw
                window_ref.request_redraw();
            }

            Event::WindowResized { physical_size, .. } => {
                // NOTE: This async path and the sync resize callback (run_resize_sync) both
                // call renderer.resize(). This is safe because:
                // 1. The sync path runs inside OS callbacks (no event loop pumping)
                // 2. The async Event::WindowResized fires after OS callback returns
                // 3. Phase 3 adds an idempotency guard to Renderer::resize() (current_size check)
                if let Some(r) = state_ref.renderer.borrow_mut().as_mut() {
                    r.resize(physical_size);
                }
            }

            Event::WindowRedrawRequested { .. } => {
                reset_borrow_order();
                run_redraw(&state_ref);
            }

            Event::WindowCloseRequested { .. } => {
                platform_ref.quit();
            }

            Event::Exiting => {
                // Clear the resize callback to release Rc<AppState> reference
                clear_render_callback();
                #[cfg(target_os = "windows")]
                clear_pump_executor_callback();
                log::info!("exiting");
            }

            Event::Wake => {
                state_ref.executor.foreground.poll();
                window_ref.request_redraw();
            }

            // -----------------------------------------------------------------
            // Mouse events
            // -----------------------------------------------------------------
            Event::MouseDown {
                position,
                button,
                modifiers,
                ..
            } => {
                let mouse_event = MouseEvent {
                    position,
                    button: Some(button),
                    modifiers,
                    timestamp: Instant::now(),
                };
                let pointer_event = PointerEvent {
                    kind: PointerEventKind::Down,
                    position,
                    button: Some(button),
                    modifiers,
                    timestamp: Instant::now(),
                };

                let bit = button_to_bit(button);
                let old_state = *state_ref.button_state.borrow();
                *state_ref.button_state.borrow_mut() = old_state | bit;

                let captured = *state_ref.capture_target.borrow();
                let target = if let Some(ct) = captured {
                    Some(ct)
                } else {
                    let hit = state_ref
                        .hit_test_list
                        .borrow()
                        .hit_test(Point::new(position.0, position.1));
                    if let Some(result) = hit {
                        *state_ref.capture_target.borrow_mut() = Some(result.element_id);
                        Some(result.element_id)
                    } else {
                        None
                    }
                };

                if let Some(t) = target {
                    bubble_mouse_handler(
                        t,
                        &mouse_event,
                        &state_ref.handler_map.borrow(),
                        &state_ref.parent_map.borrow(),
                        |h| h.on_mouse_down.clone(),
                    );

                    bubble_pointer_handler(
                        t,
                        &pointer_event,
                        &state_ref.handler_map.borrow(),
                        &state_ref.parent_map.borrow(),
                        |h| h.on_pointer_event.clone(),
                    );
                }

                *state_ref.last_mouse_pos.borrow_mut() = Some(position);
                window_ref.request_redraw();
            }

            Event::MouseUp {
                position,
                button,
                modifiers,
                ..
            } => {
                let mouse_event = MouseEvent {
                    position,
                    button: Some(button),
                    modifiers,
                    timestamp: Instant::now(),
                };
                let pointer_event = PointerEvent {
                    kind: PointerEventKind::Up,
                    position,
                    button: Some(button),
                    modifiers,
                    timestamp: Instant::now(),
                };

                let bit = button_to_bit(button);
                let old_state = *state_ref.button_state.borrow();
                let new_state = old_state & !bit;
                *state_ref.button_state.borrow_mut() = new_state;

                let captured = *state_ref.capture_target.borrow();
                let up_hit = state_ref
                    .hit_test_list
                    .borrow()
                    .hit_test(Point::new(position.0, position.1))
                    .map(|r| r.element_id);
                let target = captured.or(up_hit);

                if let Some(t) = target {
                    bubble_mouse_handler(
                        t,
                        &mouse_event,
                        &state_ref.handler_map.borrow(),
                        &state_ref.parent_map.borrow(),
                        |h| h.on_mouse_up.clone(),
                    );

                    bubble_pointer_handler(
                        t,
                        &pointer_event,
                        &state_ref.handler_map.borrow(),
                        &state_ref.parent_map.borrow(),
                        |h| h.on_pointer_event.clone(),
                    );

                    if button == MouseButton::Left && up_hit == captured && captured.is_some() {
                        bubble_mouse_handler(
                            t,
                            &mouse_event,
                            &state_ref.handler_map.borrow(),
                            &state_ref.parent_map.borrow(),
                            |h| h.on_click.clone(),
                        );
                    }
                }

                if new_state == 0 {
                    *state_ref.capture_target.borrow_mut() = None;
                }

                *state_ref.last_mouse_pos.borrow_mut() = Some(position);
                window_ref.request_redraw();
            }

            Event::MouseMoved {
                position,
                modifiers,
                ..
            } => {
                let pointer_event = PointerEvent {
                    kind: PointerEventKind::Move,
                    position,
                    button: None,
                    modifiers,
                    timestamp: Instant::now(),
                };

                let captured = *state_ref.capture_target.borrow();
                let target = if let Some(ct) = captured {
                    Some(ct)
                } else {
                    state_ref
                        .hit_test_list
                        .borrow()
                        .hit_test(Point::new(position.0, position.1))
                        .map(|r| r.element_id)
                };

                if let Some(t) = target {
                    bubble_pointer_handler(
                        t,
                        &pointer_event,
                        &state_ref.handler_map.borrow(),
                        &state_ref.parent_map.borrow(),
                        |h| h.on_pointer_event.clone(),
                    );
                }

                *state_ref.coalesced_move_pos.borrow_mut() = Some(position);
                *state_ref.last_mouse_pos.borrow_mut() = Some(position);
            }

            Event::MouseScrolled {
                position,
                delta_x,
                delta_y,
                precise,
                modifiers,
                ..
            } => {
                let scroll_event = ScrollEvent {
                    position,
                    delta_x,
                    delta_y,
                    precise,
                    modifiers,
                    timestamp: Instant::now(),
                };

                let hit = state_ref.hit_test_list.borrow().hit_test(Point::new(position.0, position.1));
                if let Some(result) = hit {
                    let target = result.element_id;

                    bubble_scroll_handler(
                        target,
                        &scroll_event,
                        &state_ref.handler_map.borrow(),
                        &state_ref.parent_map.borrow(),
                    );
                }

                window_ref.request_redraw();
            }

            Event::MouseExited { .. } => {
                let old_hover = *state_ref.hovered_element.borrow();
                if old_hover.is_some() {
                    fire_hover_transitions(
                        old_hover,
                        None,
                        &state_ref.handler_map.borrow(),
                        &state_ref.parent_map.borrow(),
                    );
                    *state_ref.hovered_element.borrow_mut() = None;
                }
                *state_ref.last_mouse_pos.borrow_mut() = None;
                *state_ref.coalesced_move_pos.borrow_mut() = None;
            }

            Event::CaptureLost { .. } => {
                *state_ref.capture_target.borrow_mut() = None;
                *state_ref.button_state.borrow_mut() = 0;
            }

            _ => {}
        });
    }
}
