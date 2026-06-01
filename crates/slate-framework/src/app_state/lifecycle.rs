//! `AppState` construction and high-level lifecycle handlers.
//!
//! - `new`: constructor; windows map starts empty — callers insert
//!   `WindowState` entries immediately after (one per platform window).
//! - `handle_wake`: drain the foreground executor on background-task wake.
//! - `handle_window_destroyed`: per-window cleanup; last-window-aware quit.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slate_platform::{
    DefaultPlatform, DefaultWindow, Window, WindowId, WindowImeDelegate, WindowOptions,
    WindowRenderDelegate, wake_run_loop,
};

use crate::app::AppContext;
use crate::executor::{Executor, RedrawRequester};
use crate::paint_cache::{TextShapingCache, TextShapingCacheObserver};
use crate::reactive_state::StateRegistry;

use super::state::{AppState, ErasedViewFactory, PendingWindowCreate};
use super::types::AppSignal;
#[cfg(any(test, feature = "test-hooks"))]
use super::types::RecoveryState;
use super::window_state::WindowState;

impl AppState {
    /// Create a new `AppState`.
    ///
    /// The `windows` map starts empty. The caller must insert a `WindowState`
    /// for the initial window immediately after construction. The reactive
    /// runtime's redraw bridge is wired here: it captures the
    /// `Arc<Mutex<Vec<RedrawRequester>>>` so it satisfies `Send + Sync` even
    /// though the `windows` `HashMap` itself is `!Sync`.
    ///
    /// Wake-all policy (v1): every registered window is woken synchronously in
    /// insertion order on each reactive signal change. Per-window subscription
    /// tagging is post-v1.
    pub fn new(
        executor: Executor,
        _redraw_requester: RedrawRequester,
        runtime: Arc<slate_reactive::Runtime>,
    ) -> Self {
        let redraw_requesters: Arc<Mutex<Vec<(WindowId, RedrawRequester)>>> =
            Arc::new(Mutex::new(Vec::new()));

        // Wire the reactive runtime's redraw bridge. The closure captures only
        // Arc<Mutex<…>> which is Send+Sync, satisfying install_redraw's bound.
        //
        // Per-window RedrawRequesters all wrap the same `wake_run_loop`
        // function (on Win32: PostMessage(WM_APP_WAKE)). Calling them in a loop
        // posts N identical wake messages per signal change. On multi-window
        // apps that combines with the wake handler (which re-posts once per
        // window) into an exponential WM_APP_WAKE flood that starves WM_PAINT.
        // Fire wake exactly once per signal change: the wake handler will
        // then InvalidateRect every live window.
        runtime.install_redraw({
            let requesters = Arc::clone(&redraw_requesters);
            Arc::new(move || {
                let guard = requesters.lock().unwrap();
                if let Some((_, req)) = guard.first() {
                    req.request();
                }
            })
        });

        let state_registry = StateRegistry::new(runtime.clone());

        // Default design tokens; `App::run` swaps `theme_set` if the adopter
        // supplied custom palettes. Mode defaults to light.
        let theme_set = RefCell::new(crate::theme::ThemeSet::default());
        let theme_mode =
            slate_reactive::Signal::new(runtime.clone(), crate::theme::ThemeMode::default());

        // Shared, atlas-independent caches.
        let text_system = Rc::new(RefCell::new(None));
        let text_shaping_cache = Rc::new(RefCell::new(TextShapingCache::new()));

        let text_shaping_cache_observer = Rc::new(TextShapingCacheObserver::new(Rc::downgrade(
            &text_shaping_cache,
        )));

        // GlyphCache + ImageCache live per-window (in WindowState) because
        // they store atlas-scoped AllocIds and each window owns its own atlas.

        Self {
            windows: RefCell::new(HashMap::new()),
            runtime,
            executor,
            text_system,
            text_shaping_cache,
            text_shaping_cache_observer,
            state_registry: RefCell::new(state_registry),
            redraw_requesters,
            pending_quit: std::cell::Cell::new(false),
            theme_set,
            theme_mode,
            platform: RefCell::new(None),
            pending_window_creates: RefCell::new(Vec::new()),
            on_key_down: RefCell::new(Vec::new()),
            on_key_up: RefCell::new(Vec::new()),
            on_text_input: RefCell::new(Vec::new()),
            on_ime_preedit: RefCell::new(Vec::new()),
            on_ime_commit: RefCell::new(Vec::new()),
            on_ime_enabled: RefCell::new(Vec::new()),
            on_ime_disabled: RefCell::new(Vec::new()),
            menu_registry: RefCell::new(crate::menu::MenuRegistry::new()),
            active_menu: RefCell::new(None),
            persistence: RefCell::new(None),
            persistence_keys: RefCell::new(HashMap::new()),
        }
    }

    /// Install the adopter's geometry-persistence store. Called once by
    /// `App::run` when the adopter supplied one via `App::persistence`.
    pub(crate) fn install_persistence(
        &self,
        store: Rc<dyn crate::persistence::PersistenceStore>,
    ) {
        *self.persistence.borrow_mut() = Some(store);
    }

    /// Apply any saved geometry onto `opts` (restore-on-create) for the
    /// dynamic `AppContext::create_window` path. Delegates to the shared
    /// helper; no-op without a store / key / saved entry.
    pub(crate) fn restore_options(&self, opts: &mut WindowOptions) {
        crate::persistence::restore_options(self.persistence.borrow().as_ref(), opts);
    }

    /// Whether the window with `id` is currently in a live resize drag. Used to
    /// gate save-on-settle so a drag does not storm the store. Unknown window
    /// (race with destroy) reports `false`.
    pub(crate) fn window_is_live_resizing(&self, id: WindowId) -> bool {
        self.windows
            .borrow()
            .get(&id)
            .map(|w| w.window.is_live_resizing())
            .unwrap_or(false)
    }

    /// Save a window's current geometry through the persistence store.
    ///
    /// No-op when no store is installed or the window has no persistence key.
    /// Reads geometry via the platform getters under a short `windows` borrow,
    /// then drops it before calling the (adopter) store — the store may run
    /// arbitrary I/O and must not alias the borrow.
    pub(crate) fn save_window_geometry(&self, id: WindowId) {
        let Some(store) = self.persistence.borrow().clone() else {
            return;
        };
        let Some(key) = self.persistence_keys.borrow().get(&id).cloned() else {
            return;
        };
        let geometry = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&id) else { return };
            crate::persistence::WindowGeometry::capture(&*win.window)
        };
        store.save(&key, geometry);
    }

    /// Register a new window's redraw requester so the reactive wake-all bridge
    /// can include it. Called after inserting the `WindowState` into `windows`.
    pub(crate) fn register_redraw_requester(&self, window: WindowId, req: RedrawRequester) {
        self.redraw_requesters.lock().unwrap().push((window, req));
    }

    /// Install the platform handle so `AppContext::create_window` can allocate
    /// new platform windows mid-dispatch. Called once by `App::run` immediately
    /// before entering the event loop.
    pub(crate) fn install_platform(&self, platform: Rc<DefaultPlatform>) {
        *self.platform.borrow_mut() = Some(platform);
    }

    /// Wire a freshly-allocated platform window into `AppState`:
    ///   1. Construct + insert `WindowState`.
    ///   2. Register a `RedrawRequester` so the reactive wake-all bridge fans
    ///      out to this window.
    ///   3. Install the shared render + IME delegates as `Weak<dyn …>` (same
    ///      Rc-clone-then-coerce dance as the first window in `App::run`).
    ///
    /// Used by both the pre-`run` `App::create_window` path and the mid-loop
    /// `AppContext::create_window` drain path. The renderer is **not** built
    /// here — `init_surfaces` does that, called by the caller after the
    /// `Event::Resumed` rendezvous (pre-run) or immediately at drain time
    /// (dynamic).
    pub(crate) fn install_window(
        self: &Rc<Self>,
        window: Arc<DefaultWindow>,
        persistence_key: Option<String>,
    ) {
        let id = window.id();

        // Remember the geometry-persistence key (if any) so save-on-settle and
        // save-on-close can store under the adopter's chosen key.
        if let Some(key) = persistence_key {
            self.persistence_keys.borrow_mut().insert(id, key);
        }

        // 1. Insert WindowState.
        let win_state = WindowState::new(window.clone(), self.runtime.clone());
        self.windows.borrow_mut().insert(id, win_state);

        // 2. Register a redraw requester for the reactive wake-all bridge.
        let redraw_requester = RedrawRequester::new(wake_run_loop);
        self.register_redraw_requester(id, redraw_requester);

        // 3. Install delegates. CANNOT use `as Weak<dyn …>` — unsizing
        // coercions don't trigger on `as`. Must Rc::clone-then-coerce
        // via let-binding, then downgrade. Identical SAFETY argument as
        // the first-window path in `App::run`.
        let dyn_strong: Rc<dyn WindowRenderDelegate> = self.clone();
        let dyn_weak = Rc::downgrade(&dyn_strong);
        window.set_render_delegate(dyn_weak);
        drop(dyn_strong);

        let ime_strong: Rc<dyn WindowImeDelegate> = self.clone();
        let ime_weak = Rc::downgrade(&ime_strong);
        window.set_ime_delegate(ime_weak);
        drop(ime_strong);
    }

    /// Drain `pending_window_creates` and finish wiring each window:
    ///   1. `install_window` (state insert + delegates).
    ///   2. `init_surfaces` (renderer + text system + view).
    ///
    /// Called by `App::run` after every event dispatch. Idempotent: an empty
    /// queue is a no-op. The drain takes ownership before iterating so a
    /// reactive effect fired by `init_surfaces` cannot re-push and deadlock.
    pub(crate) fn drain_pending_window_creates(
        self: &Rc<Self>,
        cx: &AppContext,
        platform: &DefaultPlatform,
    ) {
        let drained: Vec<PendingWindowCreate> =
            self.pending_window_creates.borrow_mut().drain(..).collect();
        for mut pending in drained {
            let id = pending.window.id();
            self.install_window(pending.window, pending.persistence_key);
            if let Err(e) = self.init_surfaces(id, &mut pending.view_factory, cx, platform) {
                log::error!(
                    "drain_pending_window_creates: init_surfaces failed for {:?}: {}",
                    id,
                    e
                );
            }
        }
    }

    /// Push a window onto the pending-create queue. Returns the `WindowId`
    /// of the (already-allocated) platform window so a caller can store it
    /// before the drain step finishes wiring it.
    pub(crate) fn push_pending_window_create(
        &self,
        window: Arc<DefaultWindow>,
        view_factory: ErasedViewFactory,
        persistence_key: Option<String>,
    ) -> WindowId {
        let id = window.id();
        self.pending_window_creates
            .borrow_mut()
            .push(PendingWindowCreate {
                window,
                view_factory,
                persistence_key,
            });
        id
    }

    /// Unregister a window's redraw requester. Called before removing the
    /// `WindowState` from `windows`.
    pub(crate) fn unregister_redraw_requester(&self, window: WindowId) {
        self.redraw_requesters
            .lock()
            .unwrap()
            .retain(|(id, _)| *id != window);
    }

    /// Handle background task completion (`Event::Wake`).
    ///
    /// Polls the foreground executor and requests a redraw on every registered
    /// window (the executor may have unblocked a reactive effect).
    pub fn handle_wake(&self) -> AppSignal {
        self.executor.foreground.poll();
        // Invalidate every live window directly (InvalidateRect / setNeedsDisplay).
        // Earlier versions called `req.request()` here, which on Win32 posts a
        // fresh WM_APP_WAKE per window — that recursively re-enters this handler
        // and on N>1 windows turns into a runaway WM_APP_WAKE flood that
        // starves WM_PAINT. Direct invalidation requests a paint tick without
        // re-posting the wake message.
        let guard = self.windows.borrow();
        for win in guard.values() {
            win.window.request_redraw();
        }
        AppSignal::None
    }

    /// Handle window close by platform (`Event::WindowDestroyed`).
    ///
    /// Removes the window's entry from `windows` and clears its redraw
    /// requester. Returns `RequestQuit` only when the last window was closed
    /// (Win32 platform-default: quit on last-window-close) or `None` on macOS
    /// (AppKit stays alive — Cmd+Q wired in the example).
    pub fn handle_window_destroyed(&self, id: WindowId) -> AppSignal {
        log::debug!("WindowDestroyed for {:?}", id);

        // Persist final geometry before the window handle is dropped. No-op
        // unless a store + key are installed for this window. Reads the window
        // while it is still in the map, so this must precede the removal below.
        self.save_window_geometry(id);
        self.persistence_keys.borrow_mut().remove(&id);

        // Drop the WindowState — renderer, view, and all per-window resources
        // are freed here. This is the only correct place to do it: the borrow
        // on `windows` is NOT held when user handlers could fire (dispatch
        // discipline), so removing here is safe.
        self.unregister_redraw_requester(id);
        self.windows.borrow_mut().remove(&id);

        let is_last = self.windows.borrow().is_empty();
        if is_last {
            #[cfg(target_os = "macos")]
            return AppSignal::None; // AppKit stays alive; example wires Cmd+Q
            #[cfg(not(target_os = "macos"))]
            return AppSignal::RequestQuit; // Win32 platform-default: quit on last-window-close
        }
        AppSignal::None
    }

    /// Request a redraw on the window with the given id.
    ///
    /// No-op (with a debug log) if the window is not found — this can happen
    /// in a race between a reactive signal change and a window destroy.
    pub(crate) fn request_redraw_for(&self, window: WindowId) {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.window.request_redraw();
        } else {
            log::debug!(
                "request_redraw_for: unknown WindowId {:?} — window may have been destroyed",
                window
            );
        }
    }

    /// Snapshot the current recovery state for a specific window. Test-only use.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn current_recovery_state_for(&self, window: WindowId) -> Option<RecoveryState> {
        self.windows
            .borrow()
            .get(&window)
            .map(|w| w.recovery_state.borrow().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{InMemoryStore, PersistenceStore, WindowGeometry};

    fn empty_state() -> Rc<AppState> {
        let redraw = RedrawRequester::new(wake_run_loop);
        let executor = crate::executor::Executor::new(redraw.clone());
        let runtime = slate_reactive::Runtime::new();
        Rc::new(AppState::new(executor, redraw, runtime))
    }

    #[test]
    fn restore_options_no_store_is_noop() {
        let state = empty_state();
        let mut opts = WindowOptions {
            size: (800, 600),
            persistence_key: Some("main".into()),
            ..Default::default()
        };
        state.restore_options(&mut opts);
        assert_eq!(opts.size, (800, 600));
    }

    #[test]
    fn restore_options_applies_saved_geometry_through_installed_store() {
        let state = empty_state();
        let store: Rc<dyn PersistenceStore> = Rc::new(InMemoryStore::new());
        store.save(
            "main",
            WindowGeometry {
                position: Some((33, 44)),
                size: (1024, 768),
                maximized: true,
            },
        );
        state.install_persistence(store);

        let mut opts = WindowOptions {
            size: (800, 600),
            position: None,
            persistence_key: Some("main".into()),
            ..Default::default()
        };
        state.restore_options(&mut opts);
        assert_eq!(opts.position, Some((33, 44)));
        assert_eq!(opts.size, (1024, 768));
        assert!(opts.maximized);
    }

    #[test]
    fn save_window_geometry_without_store_or_window_is_noop() {
        // No store + no such window: must not panic, must not insert a key.
        let state = empty_state();
        state.save_window_geometry(WindowId(999));
        assert!(state.persistence_keys.borrow().is_empty());
    }
}
