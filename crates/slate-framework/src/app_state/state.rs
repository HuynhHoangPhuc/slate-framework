//! `AppState` struct definition + `Drop` impl.
//!
//! ## Shape after per-window state lift
//!
//! Process-wide resources (reactive runtime, executor, text system,
//! image cache, app-level handler vecs) live here. Everything that is
//! naturally bound to one window now lives in
//! [`WindowState`][super::window_state::WindowState], keyed by `WindowId`
//! inside `windows`.
//!
//! ## Borrow discipline
//!
//! All outer dispatch entries take ONE `self.windows.borrow()`, resolve
//! the routed `WindowId` to a `&WindowState`, then `borrow_mut()` the
//! specific interior `RefCell`s they need. The outer `RefCell<HashMap>`
//! borrow is **released** before any user callback or platform delegate
//! is invoked. Mutations to the `HashMap` itself (window create/destroy)
//! flow through `pending_window_ops` so a handler can request a new window
//! without aliasing the outer borrow.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slate_platform::WindowId;

use crate::event::{
    ImeCommitHandler, ImeLifecycleHandler, ImePreeditHandler, KeyHandler, TextInputHandler,
};
use crate::executor::{Executor, RedrawRequester};
use crate::image_cache::{ImageCache, ImageSystemObserver};
use crate::paint_cache::{TextShapingCache, TextShapingCacheObserver};
use crate::reactive_state::StateRegistry;
use crate::text_system::{TextSystem, TextSystemObserver};

use super::window_state::WindowState;

/// Slimmed shared application state.
///
/// Generic parameter `V` has been dropped: each window's view is erased at the
/// [`WindowState`] boundary via `Box<dyn ErasedView>`, allowing heterogeneous
/// views across windows.
pub struct AppState {
    // Per-window state — keyed by WindowId.
    pub windows: RefCell<HashMap<WindowId, WindowState>>,

    // --- Process-wide resources ------------------------------------------
    pub runtime: Arc<slate_reactive::Runtime>,

    /// Foreground + background executors.
    pub executor: Executor,

    // Shared text subsystem (process-wide font cache).
    pub text_system: Rc<RefCell<Option<TextSystem>>>,
    pub text_shaping_cache: Rc<RefCell<TextShapingCache>>,

    // Device-lost cache invalidation observers (held to keep alive).
    pub text_system_observer: Rc<TextSystemObserver>,
    pub text_shaping_cache_observer: Rc<TextShapingCacheObserver>,

    // Image cache + observer (shared; content-keyed).
    pub(crate) image_cache: Rc<RefCell<ImageCache>>,
    pub(crate) image_system_observer: Rc<ImageSystemObserver>,

    // Shared per-window state registry.
    // Each window's elements are keyed by their (window-scoped) ElementId.
    // The runtime's signal-id counter is process-monotonic, so multiple
    // StateRegistry instances share one runtime without id collision.
    pub(crate) state_registry: RefCell<StateRegistry>,

    // Per-window redraw requesters held in Send+Sync storage so the reactive
    // runtime's redraw callback (which must be Send+Sync) can wake all windows
    // without touching the !Sync HashMap above. Mutated only on window
    // create/destroy; iterated synchronously in insertion order on each
    // reactive wake.
    pub redraw_requesters: Arc<Mutex<Vec<(WindowId, RedrawRequester)>>>,

    // Process-level quit flag. Producers: recovery exhaustion, last-window-close.
    pub pending_quit: std::cell::Cell<bool>,

    // App-level keyboard handler vecs (shared across windows by design).
    pub(super) on_key_down: RefCell<Vec<KeyHandler>>,
    pub(super) on_key_up: RefCell<Vec<KeyHandler>>,
    pub(super) on_text_input: RefCell<Vec<TextInputHandler>>,

    // App-level IME handler vecs.
    pub(super) on_ime_preedit: RefCell<Vec<ImePreeditHandler>>,
    pub(super) on_ime_commit: RefCell<Vec<ImeCommitHandler>>,
    pub(super) on_ime_enabled: RefCell<Vec<ImeLifecycleHandler>>,
    pub(super) on_ime_disabled: RefCell<Vec<ImeLifecycleHandler>>,
}

impl Drop for AppState {
    fn drop(&mut self) {
        log::debug!("AppState dropped — cycle-free shutdown verified");
    }
}
