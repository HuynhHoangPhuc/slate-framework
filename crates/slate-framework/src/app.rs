//! Application container and frame loop.
//!
//! `App` owns all framework resources and provides `run()` to enter
//! the platform event loop with a View.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use slate_platform::{
    DefaultPlatform, DefaultWindow, Event, Platform, Window, WindowImeDelegate, WindowOptions,
    WindowRenderDelegate, wake_run_loop,
};

use crate::app_state::{AppSignal, AppState};
use crate::event::{
    EventCtx, ImeCommitEvent, ImeCommitHandler, ImeLifecycleEvent, ImeLifecycleHandler,
    ImePreeditEvent, ImePreeditHandler, KeyEvent, KeyHandler, TextInputEvent, TextInputHandler,
};
use crate::executor::{BackgroundExecutor, Executor, RedrawRequester};
use crate::focus::FocusRegistry;
use crate::types::ElementId;
use crate::view::View;

// Synthetic device-lost trigger for Phase 2.1 closure on the real visible-window
// AppState path. Setting `SLATE_FORCE_DEVICE_LOST_MS=N` schedules a
// `force_renderer_device_lost()` N milliseconds after the first `Resumed`,
// so the OS WM_PAINT pump can be observed driving the state machine through
// `DetectedLost -> CooldownGate -> Retrying -> Recovered`.
#[cfg(all(target_os = "windows", feature = "test-hooks"))]
static FORCE_DEVICE_LOST_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(all(target_os = "windows", feature = "test-hooks"))]
static FORCE_DEVICE_LOST_ARMED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Application context passed to the view factory.
///
/// Provides access to the reactive runtime, background executor, and the
/// focus registry (Phase 9b) for constructing signals, spawning background
/// tasks, and driving focus from outside of event handlers.
///
/// Note: `ForegroundExecutor` is intentionally not exposed here because it's `!Send`
/// and bound to the UI thread. UI-thread tasks should use the foreground executor
/// available in element context methods.
#[derive(Clone)]
pub struct AppContext {
    runtime: Arc<slate_reactive::Runtime>,
    background_executor: BackgroundExecutor,
    focus_registry: Rc<RefCell<FocusRegistry>>,
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

    /// Set focus to `id` immediately (outside-handler entry point).
    ///
    /// Returns `true` when `id` is currently registered with the focus
    /// registry. Inside an event handler, prefer
    /// [`EventCtx::request_focus`](crate::event::EventCtx::request_focus) so
    /// the change is deferred until after the chain unwinds.
    pub fn set_focus(&self, id: ElementId) -> bool {
        self.focus_registry.borrow_mut().set_focus(id)
    }

    /// Clear focus immediately. Inside a handler, prefer
    /// [`EventCtx::blur`](crate::event::EventCtx::blur).
    pub fn clear_focus(&self) {
        self.focus_registry.borrow_mut().clear_focus();
    }

    /// Read the currently-focused element id.
    pub fn focused_element(&self) -> Option<ElementId> {
        self.focus_registry.borrow().focused()
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
            focus_registry: Rc::new(RefCell::new(FocusRegistry::new())),
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
    on_key_down: Vec<KeyHandler>,
    on_key_up: Vec<KeyHandler>,
    on_text_input: Vec<TextInputHandler>,
    on_ime_preedit: Vec<ImePreeditHandler>,
    on_ime_commit: Vec<ImeCommitHandler>,
    on_ime_enabled: Vec<ImeLifecycleHandler>,
    on_ime_disabled: Vec<ImeLifecycleHandler>,
}

impl App {
    /// Create a new application with the given window options.
    ///
    /// The renderer and text system are initialized lazily on `Event::Resumed`.
    pub fn new(options: WindowOptions) -> Self {
        let platform = DefaultPlatform::new();
        let window = platform.create_window(options);

        Self {
            platform,
            window,
            on_key_down: Vec::new(),
            on_key_up: Vec::new(),
            on_text_input: Vec::new(),
            on_ime_preedit: Vec::new(),
            on_ime_commit: Vec::new(),
            on_ime_enabled: Vec::new(),
            on_ime_disabled: Vec::new(),
        }
    }

    /// Register an App-level handler for `KeyDown` events. Multiple handlers
    /// may be registered; they fire in registration order. Handlers must not
    /// re-enter `App::run` or other dispatch paths — the framework holds a
    /// `RefCell` borrow on the handler vec for the duration of the call.
    ///
    /// Call before [`App::run`]; the type system enforces this (`run` consumes
    /// `self`).
    pub fn on_key_down(mut self, handler: impl FnMut(&KeyEvent, &mut EventCtx) + 'static) -> Self {
        self.on_key_down.push(Box::new(handler));
        self
    }

    /// Register an App-level handler for `KeyUp` events. See [`App::on_key_down`].
    pub fn on_key_up(mut self, handler: impl FnMut(&KeyEvent, &mut EventCtx) + 'static) -> Self {
        self.on_key_up.push(Box::new(handler));
        self
    }

    /// Register an App-level handler for composed text input. Fires once per
    /// keystroke that produces visible text (or per surrogate pair on Windows).
    /// See [`App::on_key_down`].
    pub fn on_text_input(
        mut self,
        handler: impl FnMut(&TextInputEvent, &mut EventCtx) + 'static,
    ) -> Self {
        self.on_text_input.push(Box::new(handler));
        self
    }

    /// Register an App-level handler for IME preedit (composition) updates.
    /// Fires on each `setMarkedText:` (macOS) / `WM_IME_COMPOSITION GCS_COMPSTR`
    /// (Windows). See [`App::on_key_down`].
    pub fn on_ime_preedit(
        mut self,
        handler: impl FnMut(&ImePreeditEvent, &mut EventCtx) + 'static,
    ) -> Self {
        self.on_ime_preedit.push(Box::new(handler));
        self
    }

    /// Register an App-level handler for IME commits. Empty `text` is the
    /// "clear preedit, no insert" signal; handlers MUST skip `Signal::set`
    /// in that case. See [`App::on_key_down`].
    pub fn on_ime_commit(
        mut self,
        handler: impl FnMut(&ImeCommitEvent, &mut EventCtx) + 'static,
    ) -> Self {
        self.on_ime_commit.push(Box::new(handler));
        self
    }

    /// Register an App-level handler for the start of an IME composition
    /// session. Paired with [`App::on_ime_disabled`]. See [`App::on_key_down`].
    pub fn on_ime_enabled(
        mut self,
        handler: impl FnMut(&ImeLifecycleEvent, &mut EventCtx) + 'static,
    ) -> Self {
        self.on_ime_enabled.push(Box::new(handler));
        self
    }

    /// Register an App-level handler for the end of an IME composition
    /// session. Always preceded by [`App::on_ime_enabled`]. See
    /// [`App::on_key_down`].
    pub fn on_ime_disabled(
        mut self,
        handler: impl FnMut(&ImeLifecycleEvent, &mut EventCtx) + 'static,
    ) -> Self {
        self.on_ime_disabled.push(Box::new(handler));
        self
    }

    /// Run the application with the given view factory.
    ///
    /// `view_fn` receives an [`AppContext`] with access to the reactive runtime
    /// and background executor for constructing signals and spawning tasks.
    ///
    /// This method enters the platform event loop and does not return until
    /// the application exits.
    pub fn run<V: View>(self, mut view_fn: impl FnMut(&AppContext) -> V + 'static) {
        let App {
            platform,
            window,
            on_key_down,
            on_key_up,
            on_text_input,
            on_ime_preedit,
            on_ime_commit,
            on_ime_enabled,
            on_ime_disabled,
        } = self;

        // Create executor and reactive runtime
        let redraw_requester = RedrawRequester::new(wake_run_loop);
        let executor = Executor::new(redraw_requester.clone());
        let runtime = slate_reactive::Runtime::new();

        // Capture handles needed by AppContext before AppState consumes the
        // owned originals (Executor is not Clone — only its inner
        // BackgroundExecutor is).
        let background_executor = executor.background.clone();

        // Create shared application state first so AppContext can share its
        // focus registry (Phase 9b: AppContext::set_focus / focused_element
        // need to operate on the same registry AppState dispatches against).
        let state = Rc::new(AppState::new(
            window.clone(),
            executor,
            redraw_requester,
            runtime.clone(),
        ));

        let cx = AppContext {
            runtime,
            background_executor,
            focus_registry: state.focus_registry.clone(),
        };

        // Move keyboard handlers into AppState before the platform loop starts.
        // Setter pattern (rather than ctor args) keeps test call sites stable —
        // headless harnesses that don't need keyboard dispatch never call this.
        state.install_key_handlers(on_key_down, on_key_up, on_text_input);
        state.install_ime_handlers(
            on_ime_preedit,
            on_ime_commit,
            on_ime_enabled,
            on_ime_disabled,
        );

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

        // Phase 9c: install IME delegate via the same Rc<dyn …>-via-let-binding
        // dance. Identical SAFETY argument as above (unsizing coercion needs a
        // coercion site, not an `as` cast).
        let ime_strong: Rc<dyn WindowImeDelegate> = state.clone();
        let ime_weak = Rc::downgrade(&ime_strong);
        window.set_ime_delegate(ime_weak);
        drop(ime_strong);

        let platform_ref = &platform;
        let state_ref = state.clone();

        platform.run(move |event| {
            // Check pending_quit flag from sync delegate path
            if state_ref.pending_quit.get() {
                platform_ref.quit();
                return;
            }

            // Phase 2.1 synthetic trigger: env-var-scheduled force_device_lost
            // fires from a background thread that wakes the run loop; the wake
            // event arrives here and we run on the main thread.
            #[cfg(all(target_os = "windows", feature = "test-hooks"))]
            if FORCE_DEVICE_LOST_PENDING.swap(false, std::sync::atomic::Ordering::AcqRel) {
                log::warn!(
                    target: "slate::test_hooks",
                    "SLATE_FORCE_DEVICE_LOST_MS elapsed - firing force_renderer_device_lost"
                );
                let fired = state_ref.force_renderer_device_lost();
                log::warn!(
                    target: "slate::test_hooks",
                    "force_renderer_device_lost returned fired={fired}"
                );
            }

            let signal = match event {
                Event::Resumed => {
                    if state_ref
                        .init_surfaces(&mut view_fn, &cx, platform_ref)
                        .is_err()
                    {
                        AppSignal::RequestQuit
                    } else {
                        #[cfg(all(target_os = "windows", feature = "test-hooks"))]
                        arm_force_device_lost_trigger();
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
                } => state_ref
                    .dispatch_mouse_scrolled(position, delta_x, delta_y, precise, modifiers),
                Event::MouseExited { .. } => state_ref.dispatch_mouse_exited(),
                Event::CaptureLost { .. } => state_ref.dispatch_capture_lost(),
                Event::DeviceLost { fatal, .. } => state_ref.dispatch_device_lost(fatal),
                Event::DeviceRestored { .. } => state_ref.dispatch_device_restored(),
                Event::KeyDown {
                    code,
                    key,
                    modifiers,
                    is_repeat,
                    ..
                } => state_ref.dispatch_key_down(code, key, modifiers, is_repeat),
                Event::KeyUp {
                    code,
                    key,
                    modifiers,
                    ..
                } => state_ref.dispatch_key_up(code, key, modifiers),
                Event::TextInput { text, .. } => state_ref.dispatch_text_input(text),
                Event::ImeEnabled { window, .. } => state_ref.dispatch_ime_enabled(window),
                Event::ImePreedit {
                    window,
                    text,
                    cursor_byte_offset,
                    selection,
                    ..
                } => state_ref.dispatch_ime_preedit(window, text, cursor_byte_offset, selection),
                Event::ImeCommit { window, text, .. } => {
                    state_ref.dispatch_ime_commit(window, text)
                }
                Event::ImeDisabled { window, .. } => state_ref.dispatch_ime_disabled(window),
                Event::Exiting => {
                    log::info!("exiting");
                    AppSignal::None
                }
                // Slate's Event is #[non_exhaustive]; this final arm preserves
                // forward compatibility — future Slate variants are observed
                // but not yet routed.
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

/// Read `SLATE_FORCE_DEVICE_LOST_MS` once and arm a background thread that
/// sleeps the requested duration, sets `FORCE_DEVICE_LOST_PENDING`, and
/// wakes the run loop. The wake handler on the main thread observes the
/// flag and invokes `AppState::force_renderer_device_lost`.
///
/// Subsequent calls are no-ops (one-shot arming).
#[cfg(all(target_os = "windows", feature = "test-hooks"))]
fn arm_force_device_lost_trigger() {
    use std::sync::atomic::Ordering;

    if FORCE_DEVICE_LOST_ARMED.swap(true, Ordering::AcqRel) {
        return;
    }

    let ms: u64 = match std::env::var("SLATE_FORCE_DEVICE_LOST_MS") {
        Ok(s) => match s.trim().parse() {
            Ok(n) => n,
            Err(_) => {
                log::warn!(
                    target: "slate::test_hooks",
                    "SLATE_FORCE_DEVICE_LOST_MS='{s}' is not a valid u64 - skipping"
                );
                return;
            }
        },
        Err(_) => return,
    };

    log::warn!(
        target: "slate::test_hooks",
        "SLATE_FORCE_DEVICE_LOST_MS armed: will fire force_renderer_device_lost in {ms}ms"
    );

    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(ms));
        FORCE_DEVICE_LOST_PENDING.store(true, Ordering::Release);
        wake_run_loop();
    });
}
