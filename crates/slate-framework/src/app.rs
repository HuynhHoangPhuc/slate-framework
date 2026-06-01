//! Application container and frame loop.
//!
//! `App` owns all framework resources and provides `run()` to enter
//! the platform event loop with a View.

use std::rc::Rc;
use std::sync::Arc;

use slate_platform::{
    DefaultPlatform, DefaultWindow, Event, Platform, Window, WindowId, WindowOptions, wake_run_loop,
};

use crate::app_state::{AppSignal, AppState, ErasedViewFactory};
use crate::erased_view::ErasedView;
use crate::event::{
    EventCtx, ImeCommitEvent, ImeCommitHandler, ImeLifecycleEvent, ImeLifecycleHandler,
    ImePreeditEvent, ImePreeditHandler, KeyEvent, KeyHandler, TextInputEvent, TextInputHandler,
};
use crate::executor::{BackgroundExecutor, Executor, RedrawRequester};
use crate::types::ElementId;
use crate::view::View;

// Synthetic device-lost trigger on the real visible-window AppState path.
// Setting `SLATE_FORCE_DEVICE_LOST_MS=N` schedules a
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
/// Provides access to the reactive runtime and background executor for
/// constructing signals and spawning background tasks.
///
/// Focus operations are now per-window; use `set_focus(window, id)` etc.
///
/// Note: `ForegroundExecutor` is intentionally not exposed here because it's `!Send`
/// and bound to the UI thread. UI-thread tasks should use the foreground executor
/// available in element context methods.
#[derive(Clone)]
pub struct AppContext {
    pub(crate) runtime: Arc<slate_reactive::Runtime>,
    pub(crate) background_executor: BackgroundExecutor,
    /// Weak reference back to AppState so focus API can route per-window.
    /// `None` only in unit-test construction via `new_for_test`.
    pub(crate) state: Option<std::rc::Weak<AppState>>,
}

impl AppContext {
    /// Get the reactive runtime for creating signals.
    pub fn runtime(&self) -> Arc<slate_reactive::Runtime> {
        self.runtime.clone()
    }

    /// Open a new window from inside an event handler.
    ///
    /// The platform window (HWND on Win32, NSWindow on AppKit) is allocated
    /// **synchronously** so we can return a stable [`WindowId`] right away.
    /// The framework-side bring-up — `WindowState` insertion, delegate
    /// install, renderer + view init — is **deferred** to the dispatch drain
    /// step at the end of the current event so it never aliases the
    /// `windows.borrow()` already taken by the in-flight handler.
    ///
    /// Returns `None` only when called from a context where the framework
    /// has dropped its platform handle (post-`run`, or a unit-test
    /// `AppContext::new_for_test`).
    pub fn create_window<V: View + 'static>(
        &self,
        opts: WindowOptions,
        mut view_fn: impl FnMut(&AppContext) -> V + 'static,
    ) -> Option<WindowId> {
        let weak = self.state.as_ref()?;
        let state = weak.upgrade()?;
        let platform_rc = state.platform.borrow().clone()?;
        let window = platform_rc.create_window(opts);
        let cx_clone = self.clone();
        let factory: ErasedViewFactory = Box::new(move |_cx: &AppContext| -> Box<dyn ErasedView> {
            Box::new(view_fn(&cx_clone))
        });
        Some(state.push_pending_window_create(window, factory))
    }

    /// Get the background executor for spawning async tasks.
    pub fn background_executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }

    /// Request the run loop to exit at the next event tick.
    ///
    /// Sets the shared `pending_quit` flag that `App::run` checks before each
    /// dispatch. Idempotent. Required by examples that drive their own quit
    /// affordance (`AppKit` keeps the app alive after last-window-close, so
    /// macOS examples must wire Cmd+Q explicitly). No-op outside of a live
    /// `App::run` (e.g. `new_for_test` constructions).
    pub fn request_quit(&self) {
        let Some(weak) = &self.state else { return };
        let Some(state) = weak.upgrade() else { return };
        state.pending_quit.set(true);
    }

    /// Set focus to `id` in the given window immediately (outside-handler
    /// entry point).
    ///
    /// Returns `true` when `id` is currently registered with the focus
    /// registry. Inside an event handler, prefer
    /// [`EventCtx::request_focus`](crate::event::EventCtx::request_focus) so
    /// the change is deferred until after the chain unwinds.
    pub fn set_focus(&self, window: slate_platform::WindowId, id: ElementId) -> bool {
        let Some(weak) = &self.state else {
            return false;
        };
        let Some(state) = weak.upgrade() else {
            return false;
        };
        let guard = state.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.focus_registry.borrow_mut().set_focus(id)
        } else {
            false
        }
    }

    /// Clear focus immediately in the given window. Inside a handler, prefer
    /// [`EventCtx::blur`](crate::event::EventCtx::blur).
    pub fn clear_focus(&self, window: slate_platform::WindowId) {
        let Some(weak) = &self.state else { return };
        let Some(state) = weak.upgrade() else { return };
        let guard = state.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.focus_registry.borrow_mut().clear_focus();
        }
    }

    /// Read the currently-focused element id in the given window.
    pub fn focused_element(&self, window: slate_platform::WindowId) -> Option<ElementId> {
        let weak = self.state.as_ref()?;
        let state = weak.upgrade()?;
        let guard = state.windows.borrow();
        guard.get(&window)?.focus_registry.borrow().focused()
    }

    /// The app-global theme-mode signal (light/dark).
    ///
    /// Hold the returned [`Signal`](slate_reactive::Signal) in your view and
    /// flip it from an event handler to switch palettes; the change re-renders
    /// the whole view (Strategy A). Returns `None` only in test constructions
    /// without an [`AppState`].
    ///
    /// ```ignore
    /// let mode = cx.theme_mode().unwrap();
    /// // in a handler:
    /// mode.update(|m| *m = match *m {
    ///     ThemeMode::Light => ThemeMode::Dark,
    ///     ThemeMode::Dark => ThemeMode::Light,
    /// });
    /// ```
    pub fn theme_mode(&self) -> Option<slate_reactive::Signal<crate::theme::ThemeMode>> {
        let state = self.state.as_ref()?.upgrade()?;
        Some(state.theme_mode.clone())
    }

    /// Install `menu` as the application's native menu bar.
    ///
    /// The model is stored and lowered to a platform descriptor, then pushed to
    /// every window through the platform menu seam. Until a native backend is
    /// wired (P6b macOS / P6c Windows) the platform call is a no-op, but the
    /// model + action routing are fully live and testable. Register a handler
    /// for each actionable item's [`MenuId`](crate::MenuId) via
    /// [`on_menu_action`](Self::on_menu_action).
    ///
    /// No-op outside a live [`App::run`] (e.g. `new_for_test`).
    pub fn set_menu(&self, menu: crate::menu::Menu) {
        let Some(weak) = &self.state else { return };
        let Some(state) = weak.upgrade() else { return };
        state.install_menu(menu);
    }

    /// Register the handler invoked when the menu item carrying `id` is
    /// activated. Replaces any prior handler for the same id.
    ///
    /// Handlers run on the main thread; capture `Signal`/`AppContext` clones to
    /// mutate state — the mutation schedules the next view rebuild (Strategy A),
    /// exactly like an `on_click`/key handler. No-op outside a live
    /// [`App::run`].
    ///
    /// Handler registration is **independent of menu installation** and in any
    /// order: register before or after [`set_menu`](Self::set_menu). Handlers
    /// persist across menu swaps (installing a new menu does not clear them),
    /// so apps with dynamic menus should reuse stable [`MenuId`](crate::MenuId)s
    /// and overwrite a handler by re-registering its id. Activating an id with
    /// no registered handler is a safe no-op.
    pub fn on_menu_action(&self, id: crate::menu::MenuId, handler: impl Fn() + 'static) {
        let Some(weak) = &self.state else { return };
        let Some(state) = weak.upgrade() else { return };
        state.register_menu_action(id, handler);
    }

    /// Construct an `AppContext` directly. Test-only.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn new_for_test(
        runtime: Arc<slate_reactive::Runtime>,
        background_executor: BackgroundExecutor,
    ) -> Self {
        Self {
            runtime,
            background_executor,
            state: None,
        }
    }
}

/// A window registered before `App::run` but not yet wired into `AppState`.
/// Built by `App::create_window`, materialised inside `App::run` alongside
/// the first window.
struct PendingPreRunWindow {
    window: Arc<DefaultWindow>,
    view_factory: ErasedViewFactory,
}

/// Application container.
///
/// Owns all framework resources: platform, window, executor,
/// layout tree, hit-test list, accessibility tree, and text system.
///
/// # Examples
///
/// ```ignore
/// use slate_framework::{App, WindowOptions, Text, View, RenderCx, AnyElement, IntoAny};
///
/// struct HelloView;
/// impl View for HelloView {
///     fn render(&mut self, _cx: &mut RenderCx) -> AnyElement {
///         Text::new("Hello, Slate!").into_any()
///     }
/// }
///
/// App::new(WindowOptions::default()).run(|_cx| HelloView);
/// ```
pub struct App {
    platform: Rc<DefaultPlatform>,
    window: Arc<DefaultWindow>,
    pending_windows: Vec<PendingPreRunWindow>,
    on_key_down: Vec<KeyHandler>,
    on_key_up: Vec<KeyHandler>,
    on_text_input: Vec<TextInputHandler>,
    on_ime_preedit: Vec<ImePreeditHandler>,
    on_ime_commit: Vec<ImeCommitHandler>,
    on_ime_enabled: Vec<ImeLifecycleHandler>,
    on_ime_disabled: Vec<ImeLifecycleHandler>,
    themes: Option<crate::theme::ThemeSet>,
}

impl App {
    /// Create a new application with the given window options.
    ///
    /// The renderer and text system are initialized lazily on `Event::Resumed`.
    pub fn new(options: WindowOptions) -> Self {
        let platform = Rc::new(DefaultPlatform::new());
        let window = platform.create_window(options);

        Self {
            platform,
            window,
            pending_windows: Vec::new(),
            on_key_down: Vec::new(),
            on_key_up: Vec::new(),
            on_text_input: Vec::new(),
            on_ime_preedit: Vec::new(),
            on_ime_commit: Vec::new(),
            on_ime_enabled: Vec::new(),
            on_ime_disabled: Vec::new(),
            themes: None,
        }
    }

    /// Install custom light and dark palettes.
    ///
    /// Build each from [`Theme::light`](crate::Theme::light) /
    /// [`Theme::dark`](crate::Theme::dark) and tweak fields. When unset, the
    /// framework defaults are used. Call before [`App::run`].
    pub fn theme(mut self, light: crate::theme::Theme, dark: crate::theme::Theme) -> Self {
        self.themes = Some(crate::theme::ThemeSet::new(light, dark));
        self
    }

    /// Open an additional window before entering the run loop.
    ///
    /// The platform window is allocated immediately so the returned
    /// [`WindowId`] is usable straight away (e.g. capturing it in a handler
    /// closure registered via [`App::on_key_down`]). `WindowState` insertion,
    /// delegate install, and renderer init are deferred until inside
    /// [`App::run`] so the first window and any pre-run extras come up in a
    /// single rendezvous on `Event::Resumed`.
    ///
    /// The `view_fn` is erased to `Box<dyn ErasedView>` at registration so
    /// `AppState` does not need to be generic over `V`.
    pub fn create_window<V: View + 'static>(
        &mut self,
        opts: WindowOptions,
        mut view_fn: impl FnMut(&AppContext) -> V + 'static,
    ) -> WindowId {
        let window = self.platform.create_window(opts);
        let id = window.id();
        let factory: ErasedViewFactory =
            Box::new(move |cx: &AppContext| -> Box<dyn ErasedView> { Box::new(view_fn(cx)) });
        self.pending_windows.push(PendingPreRunWindow {
            window,
            view_factory: factory,
        });
        id
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
            pending_windows,
            on_key_down,
            on_key_up,
            on_text_input,
            on_ime_preedit,
            on_ime_commit,
            on_ime_enabled,
            on_ime_disabled,
            themes,
        } = self;

        // Create executor and reactive runtime.
        let redraw_requester = RedrawRequester::new(wake_run_loop);
        let executor = Executor::new(redraw_requester.clone());
        let runtime = slate_reactive::Runtime::new();

        // Capture handles needed by AppContext before AppState consumes them.
        let background_executor = executor.background.clone();

        // Create shared application state. windows map starts empty.
        let state = Rc::new(AppState::new(
            executor,
            redraw_requester.clone(),
            runtime.clone(),
        ));

        // Swap in adopter-supplied palettes, if any (default otherwise).
        if let Some(set) = themes {
            state.theme_set.replace(set);
        }

        // Install platform handle so `AppContext::create_window` can allocate
        // additional platform windows mid-dispatch.
        state.install_platform(platform.clone());

        // Wire the FIRST window (its WindowState, redraw requester, and
        // render+IME delegates) using the same helper the dynamic
        // `AppContext::create_window` path uses.
        let first_window_id = window.id();
        state.install_window(window.clone());

        let cx = AppContext {
            runtime,
            background_executor,
            state: Some(Rc::downgrade(&state)),
        };

        // Move keyboard/IME handlers into AppState.
        state.install_key_handlers(on_key_down, on_key_up, on_text_input);
        state.install_ime_handlers(
            on_ime_preedit,
            on_ime_commit,
            on_ime_enabled,
            on_ime_disabled,
        );

        // Wrap the FIRST window's typed view factory into a type-erased one.
        let cx_clone = cx.clone();
        let first_erased: ErasedViewFactory =
            Box::new(move |_cx: &AppContext| -> Box<dyn ErasedView> {
                Box::new(view_fn(&cx_clone))
            });

        // Wire each pre-`run` window (registered via `App::create_window`) and
        // build the (id, erased_factory) list consumed on Event::Resumed.
        let mut resumed_factories: Vec<(WindowId, ErasedViewFactory)> =
            Vec::with_capacity(1 + pending_windows.len());
        resumed_factories.push((first_window_id, first_erased));
        for pre in pending_windows {
            let id = pre.window.id();
            state.install_window(pre.window);
            resumed_factories.push((id, pre.view_factory));
        }

        let platform_ref = &*platform;
        let state_ref = state.clone();

        platform.run(move |event| {
            // Check pending_quit flag from sync delegate path.
            if state_ref.pending_quit.get() {
                platform_ref.quit();
                return;
            }

            // Synthetic trigger: env-var-scheduled force_device_lost fires from
            // a background thread that wakes the run loop.
            #[cfg(all(target_os = "windows", feature = "test-hooks"))]
            if FORCE_DEVICE_LOST_PENDING.swap(false, std::sync::atomic::Ordering::AcqRel) {
                log::warn!(
                    target: "slate::test_hooks",
                    "SLATE_FORCE_DEVICE_LOST_MS elapsed - firing force_renderer_device_lost"
                );
                let fired = state_ref.force_renderer_device_lost(first_window_id);
                log::warn!(
                    target: "slate::test_hooks",
                    "force_renderer_device_lost returned fired={fired}"
                );
            }

            let signal = match event {
                Event::Resumed => {
                    // Initialise renderer + view for every window registered
                    // before `run` (first + any `App::create_window` extras).
                    // If ANY window fails to come up, request quit.
                    let mut had_failure = false;
                    for (id, factory) in resumed_factories.iter_mut() {
                        if state_ref
                            .init_surfaces(*id, factory, &cx, platform_ref)
                            .is_err()
                        {
                            had_failure = true;
                        }
                    }
                    if had_failure {
                        AppSignal::RequestQuit
                    } else {
                        #[cfg(all(target_os = "windows", feature = "test-hooks"))]
                        arm_force_device_lost_trigger();
                        AppSignal::RequestRedraw {
                            window: first_window_id,
                        }
                    }
                }
                Event::WindowResized {
                    window,
                    physical_size,
                    ..
                } => {
                    state_ref.handle_window_resized(window, physical_size);
                    AppSignal::None
                }
                Event::WindowRedrawRequested { window, .. } => state_ref.dispatch_redraw(window),
                Event::WindowCloseRequested { .. } => {
                    // Let the platform proceed with destroy; the last-window
                    // quit decision is owned by `handle_window_destroyed`,
                    // which sees the post-destroy windows map and applies the
                    // platform-default policy (Win32 quits, AppKit stays alive).
                    AppSignal::None
                }
                Event::WindowDestroyed { window, .. } => state_ref.handle_window_destroyed(window),
                Event::Wake => state_ref.handle_wake(),
                Event::MouseDown {
                    window,
                    position,
                    button,
                    modifiers,
                    ..
                } => state_ref.dispatch_mouse_down(window, position, button, modifiers),
                Event::MouseUp {
                    window,
                    position,
                    button,
                    modifiers,
                    ..
                } => state_ref.dispatch_mouse_up(window, position, button, modifiers),
                Event::MouseMoved {
                    window,
                    position,
                    modifiers,
                    ..
                } => state_ref.dispatch_mouse_moved(window, position, modifiers),
                Event::MouseScrolled {
                    window,
                    position,
                    delta_x,
                    delta_y,
                    precise,
                    modifiers,
                    ..
                } => state_ref.dispatch_mouse_scrolled(
                    window, position, delta_x, delta_y, precise, modifiers,
                ),
                Event::MouseExited { window, .. } => state_ref.dispatch_mouse_exited(window),
                Event::CaptureLost { window, .. } => state_ref.dispatch_capture_lost(window),
                Event::DeviceLost { window, fatal, .. } => {
                    state_ref.dispatch_device_lost(window, fatal)
                }
                Event::DeviceRestored { window, .. } => state_ref.dispatch_device_restored(window),
                Event::KeyDown {
                    window,
                    code,
                    key,
                    modifiers,
                    is_repeat,
                    ..
                } => state_ref.dispatch_key_down(window, code, key, modifiers, is_repeat),
                Event::KeyUp {
                    window,
                    code,
                    key,
                    modifiers,
                    ..
                } => state_ref.dispatch_key_up(window, code, key, modifiers),
                Event::TextInput { window, text, .. } => {
                    state_ref.dispatch_text_input(window, text)
                }
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
                // forward compatibility.
                _ => AppSignal::None,
            };

            match signal {
                AppSignal::RequestQuit => platform_ref.quit(),
                AppSignal::RequestRedraw { window } => state_ref.request_redraw_for(window),
                AppSignal::None => {}
            }

            // Drain any windows requested mid-dispatch via
            // `AppContext::create_window`. Borrow on `windows` taken by the
            // handler has been released by this point.
            state_ref.drain_pending_window_creates(&cx, platform_ref);
        });
    }
}

/// Read `SLATE_FORCE_DEVICE_LOST_MS` once and arm a background thread that
/// sleeps the requested duration, sets `FORCE_DEVICE_LOST_PENDING`, and
/// wakes the run loop.
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
