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

use slate_platform::{Window, WindowId};

use crate::executor::{Executor, RedrawRequester};
use crate::image_cache::{ImageCache, ImageSystemObserver};
use crate::paint_cache::{TextShapingCache, TextShapingCacheObserver};
use crate::reactive_state::StateRegistry;
use crate::text_system::TextSystemObserver;

use super::state::AppState;
use super::types::AppSignal;
#[cfg(any(test, feature = "test-hooks"))]
use super::types::RecoveryState;

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
    pub fn new(executor: Executor, _redraw_requester: RedrawRequester, runtime: Arc<slate_reactive::Runtime>) -> Self {
        let redraw_requesters: Arc<Mutex<Vec<(WindowId, RedrawRequester)>>> =
            Arc::new(Mutex::new(Vec::new()));

        // Wire the reactive runtime's redraw bridge. The closure captures only
        // Arc<Mutex<…>> which is Send+Sync, satisfying install_redraw's bound.
        // On fire it walks all registered windows in insertion order and calls
        // request() synchronously on each — cheap (non-blocking PostMessage /
        // setNeedsDisplay) and deterministic for golden-trace stability.
        runtime.install_redraw({
            let requesters = Arc::clone(&redraw_requesters);
            Arc::new(move || {
                let guard = requesters.lock().unwrap();
                for (_, req) in guard.iter() {
                    req.request();
                }
            })
        });

        let state_registry = StateRegistry::new(runtime.clone());

        // Create Rc-wrapped caches for observer weak references.
        let text_system = Rc::new(RefCell::new(None));
        let text_shaping_cache = Rc::new(RefCell::new(TextShapingCache::new()));

        let text_system_observer = Rc::new(TextSystemObserver::new(Rc::downgrade(&text_system)));
        let text_shaping_cache_observer = Rc::new(TextShapingCacheObserver::new(Rc::downgrade(
            &text_shaping_cache,
        )));

        let image_cache = Rc::new(RefCell::new(ImageCache::new()));
        let image_system_observer = Rc::new(ImageSystemObserver::new(Rc::downgrade(&image_cache)));

        Self {
            windows: RefCell::new(HashMap::new()),
            runtime,
            executor,
            text_system,
            text_shaping_cache,
            text_system_observer,
            text_shaping_cache_observer,
            image_cache,
            image_system_observer,
            state_registry: RefCell::new(state_registry),
            redraw_requesters,
            pending_quit: std::cell::Cell::new(false),
            on_key_down: RefCell::new(Vec::new()),
            on_key_up: RefCell::new(Vec::new()),
            on_text_input: RefCell::new(Vec::new()),
            on_ime_preedit: RefCell::new(Vec::new()),
            on_ime_commit: RefCell::new(Vec::new()),
            on_ime_enabled: RefCell::new(Vec::new()),
            on_ime_disabled: RefCell::new(Vec::new()),
        }
    }

    /// Register a new window's redraw requester so the reactive wake-all bridge
    /// can include it. Called after inserting the `WindowState` into `windows`.
    pub(crate) fn register_redraw_requester(&self, window: WindowId, req: RedrawRequester) {
        self.redraw_requesters.lock().unwrap().push((window, req));
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
        // Wake all live windows so any reactive effect that just unblocked gets
        // a paint tick. The platform's request_redraw is idempotent and cheap.
        let guard = self.redraw_requesters.lock().unwrap();
        for (_, req) in guard.iter() {
            req.request();
        }
        drop(guard);
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
