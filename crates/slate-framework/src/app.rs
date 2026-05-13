//! Application container and frame loop.
//!
//! `App` owns all framework resources and provides `run()` to enter
//! the platform event loop with a View.

use std::rc::Rc;
use std::sync::Arc;

use slate_platform::{
    DefaultPlatform, DefaultWindow, Event, Platform, Window, WindowOptions, WindowRenderDelegate,
    wake_run_loop,
};

use crate::app_state::{AppSignal, AppState};
use crate::executor::{BackgroundExecutor, Executor, RedrawRequester};
use crate::view::View;

/// Application context passed to the view factory.
///
/// Provides access to the reactive runtime and background executor for constructing
/// signals and spawning background tasks.
///
/// Note: `ForegroundExecutor` is intentionally not exposed here because it's `!Send`
/// and bound to the UI thread. UI-thread tasks should use the foreground executor
/// available in element context methods.
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

    /// Construct an `AppContext` directly. Test-only.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn new_for_test(
        runtime: Arc<slate_reactive::Runtime>,
        background_executor: BackgroundExecutor,
    ) -> Self {
        Self {
            runtime,
            background_executor,
        }
    }
}

/// Application container.
///
/// Owns all framework resources: platform, window, renderer, executor,
/// layout tree, hit-test list, accessibility tree, and text system.
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

        // Create shared application state
        let state = Rc::new(AppState::new(
            window.clone(),
            executor,
            redraw_requester,
            runtime,
        ));

        // Install render delegate on the platform window.
        //
        // CANNOT write: `Rc::downgrade(&state) as Weak<dyn WindowRenderDelegate>`.
        // Per Rust Reference (unsized coercions): unsizing coercions are triggered
        // in coercion sites (let-bindings, fn args, struct fields) but NOT by `as`
        // casts. The ONLY working path: build an explicit Rc<dyn WindowRenderDelegate>
        // via Rc::clone-then-coerce in a let-binding, then downgrade it.
        let dyn_strong: Rc<dyn WindowRenderDelegate> = state.clone();
        let dyn_weak = Rc::downgrade(&dyn_strong);
        window.set_render_delegate(dyn_weak);
        drop(dyn_strong); // strong ref no longer needed; `state` keeps AppState alive.

        let platform_ref = &platform;
        let state_ref = state.clone();

        platform.run(move |event| {
            // Check pending_quit flag from sync delegate path
            if state_ref.pending_quit.get() {
                platform_ref.quit();
                return;
            }

            let signal = match event {
                Event::Resumed => {
                    if state_ref.init_surfaces(&mut view_fn, &cx, platform_ref).is_err() {
                        AppSignal::RequestQuit
                    } else {
                        AppSignal::RequestRedraw
                    }
                }
                Event::WindowResized { physical_size, .. } => {
                    state_ref.handle_window_resized(physical_size);
                    AppSignal::None
                }
                Event::WindowRedrawRequested { .. } => state_ref.dispatch_redraw(),
                Event::WindowCloseRequested { .. } => AppSignal::RequestQuit,
                Event::WindowDestroyed { .. } => state_ref.handle_window_destroyed(),
                Event::Wake => state_ref.handle_wake(),
                Event::MouseDown {
                    position,
                    button,
                    modifiers,
                    ..
                } => state_ref.dispatch_mouse_down(position, button, modifiers),
                Event::MouseUp {
                    position,
                    button,
                    modifiers,
                    ..
                } => state_ref.dispatch_mouse_up(position, button, modifiers),
                Event::MouseMoved {
                    position,
                    modifiers,
                    ..
                } => state_ref.dispatch_mouse_moved(position, modifiers),
                Event::MouseScrolled {
                    position,
                    delta_x,
                    delta_y,
                    precise,
                    modifiers,
                    ..
                } => state_ref.dispatch_mouse_scrolled(position, delta_x, delta_y, precise, modifiers),
                Event::MouseExited { .. } => state_ref.dispatch_mouse_exited(),
                Event::CaptureLost { .. } => state_ref.dispatch_capture_lost(),
                Event::DeviceLost { fatal, .. } => state_ref.dispatch_device_lost(fatal),
                Event::DeviceRestored { .. } => state_ref.dispatch_device_restored(),
                Event::Exiting => {
                    log::info!("exiting");
                    AppSignal::None
                }
                _ => AppSignal::None,
            };

            match signal {
                AppSignal::RequestQuit => platform_ref.quit(),
                AppSignal::RequestRedraw => state_ref.window.request_redraw(),
                AppSignal::None => {}
            }
        });
    }
}
