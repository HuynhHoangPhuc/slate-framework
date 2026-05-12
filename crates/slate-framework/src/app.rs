//! Application container and frame loop.
//!
//! `App` owns all framework resources and provides `run()` to enter
//! the platform event loop with a View.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use slate_platform::{
    DefaultPlatform, DefaultWindow, Event, Platform, Window, WindowOptions, wake_run_loop,
};

use crate::app_state::{
    AppSignal, AppState, bubble_mouse_handler, bubble_pointer_handler, bubble_scroll_handler,
    button_to_bit, fire_hover_transitions,
};
use crate::event::{MouseEvent, PointerEvent, PointerEventKind, ScrollEvent};
use crate::executor::{BackgroundExecutor, Executor, RedrawRequester};
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

        // Create executor and reactive runtime
        let redraw_requester = RedrawRequester::new(wake_run_loop);
        let executor = Executor::new(redraw_requester.clone());
        let runtime = slate_reactive::Runtime::new();

        // AppContext for view factory
        let cx = AppContext {
            runtime: runtime.clone(),
            background_executor: executor.background.clone(),
        };

        // Create shared application state (Phase 1: all state lives in AppState)
        let state = Rc::new(AppState::new(
            window.clone(),
            executor,
            redraw_requester,
            runtime,
        ));

        let platform_ref = &platform;
        let state_ref = state.clone();

        platform.run(move |event| {
            // Check pending_quit flag from sync delegate path
            if state_ref.pending_quit.get() {
                platform_ref.quit();
                return;
            }

            match event {
                Event::Resumed => {
                    let _ = state_ref.init_surfaces(&mut view_fn, &cx, platform_ref);
                }

                Event::WindowResized { physical_size, .. } => {
                    state_ref.handle_window_resized(physical_size);
                }

                Event::WindowRedrawRequested { .. } => {
                    if matches!(state_ref.dispatch_redraw(), AppSignal::RequestQuit) {
                        platform_ref.quit();
                    }
                }

                Event::WindowCloseRequested { .. } => {
                    platform_ref.quit();
                }

                Event::Exiting => {
                    log::info!("exiting");
                }

                Event::Wake => {
                    // Background task completed — poll executor to process results,
                    // then request redraw so any state changes render this frame.
                    state_ref.executor.foreground.poll();
                    state_ref.window.request_redraw();
                }

                // -----------------------------------------------------------------
                // Mouse events (Phase 5a) — will be lifted to AppState methods in Phase 2
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

                    // Update button state
                    let bit = button_to_bit(button);
                    let old_state = *state_ref.button_state.borrow();
                    *state_ref.button_state.borrow_mut() = old_state | bit;

                    // Determine dispatch target
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
                    state_ref.window.request_redraw();
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

                    // Update button state
                    let bit = button_to_bit(button);
                    let old_state = *state_ref.button_state.borrow();
                    let new_state = old_state & !bit;
                    *state_ref.button_state.borrow_mut() = new_state;

                    // Determine dispatch target
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

                        // Click synthesis
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

                    // Release capture when all buttons are up
                    if new_state == 0 {
                        *state_ref.capture_target.borrow_mut() = None;
                    }

                    *state_ref.last_mouse_pos.borrow_mut() = Some(position);
                    state_ref.window.request_redraw();
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

                    // Route through capture_target if captured, else hit-test
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

                    let hit = state_ref
                        .hit_test_list
                        .borrow()
                        .hit_test(Point::new(position.0, position.1));
                    if let Some(result) = hit {
                        bubble_scroll_handler(
                            result.element_id,
                            &scroll_event,
                            &state_ref.handler_map.borrow(),
                            &state_ref.parent_map.borrow(),
                        );
                    }

                    state_ref.window.request_redraw();
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

                // -----------------------------------------------------------------
                // Device recovery events (Phase 5)
                // -----------------------------------------------------------------
                Event::DeviceLost { fatal, .. } => {
                    if fatal {
                        log::error!("GPU device lost (fatal) - recovery failed after max attempts");
                        platform_ref.quit();
                    } else {
                        log::warn!("GPU device lost - recovery will be attempted");
                    }
                }

                Event::DeviceRestored { .. } => {
                    log::info!("GPU device restored - rendering resumed");
                    state_ref.window.request_redraw();
                }

                _ => {}
            }
        });
    }
}
