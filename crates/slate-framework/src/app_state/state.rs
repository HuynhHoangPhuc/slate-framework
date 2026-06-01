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

use slate_platform::{DefaultPlatform, DefaultWindow, WindowId};

use crate::app::AppContext;
use crate::erased_view::ErasedView;
use crate::event::{
    ImeCommitHandler, ImeLifecycleHandler, ImePreeditHandler, KeyHandler, TextInputHandler,
};
use crate::executor::{Executor, RedrawRequester};
use crate::paint_cache::{TextShapingCache, TextShapingCacheObserver};
use crate::reactive_state::StateRegistry;
use crate::text_system::TextSystem;

use super::window_state::WindowState;

/// Type-erased view factory used both for the initial window's lazy `Event::Resumed`
/// init and for windows pushed onto `pending_window_creates` by
/// `AppContext::create_window`.
pub(crate) type ErasedViewFactory = Box<dyn FnMut(&AppContext) -> Box<dyn ErasedView>>;

/// A window that has been created (HWND/NSWindow allocated) but not yet
/// fully wired into `windows`/`redraw_requesters`/delegates. Drained inside
/// `App::run` after each event dispatch.
pub(crate) struct PendingWindowCreate {
    pub window: Arc<DefaultWindow>,
    pub view_factory: ErasedViewFactory,
}

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
    //
    // Note: `TextShapingCache` is atlas-independent (stores positioned glyph
    // IDs only) and remains shared. `GlyphCache` and `ImageCache` are NOT
    // shared — they hold atlas-scoped `AllocId`s, so each window owns its own
    // in `WindowState`.
    pub text_system: Rc<RefCell<Option<TextSystem>>>,
    pub text_shaping_cache: Rc<RefCell<TextShapingCache>>,

    // Device-lost cache invalidation observer for `text_shaping_cache` (the
    // only remaining shared cache that needs clearing on renderer recreate).
    pub text_shaping_cache_observer: Rc<TextShapingCacheObserver>,

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

    // Ambient design tokens (process-wide). `theme_set` holds the light/dark
    // palettes (swapped once at `App::run` if the adopter supplied custom
    // ones); `theme_mode` is read by every `theme()` call during render so
    // flipping it re-renders the view (Strategy A).
    pub(crate) theme_set: RefCell<crate::theme::ThemeSet>,
    pub(crate) theme_mode: slate_reactive::Signal<crate::theme::ThemeMode>,

    // Platform handle, installed by `App::run` immediately before entering
    // the event loop. `None` for unit tests that exercise AppState without
    // a real platform. Used by `AppContext::create_window` so a handler can
    // request a new HWND/NSWindow without piping the platform through every
    // dispatch path.
    pub(crate) platform: RefCell<Option<Rc<DefaultPlatform>>>,

    // Windows allocated by `AppContext::create_window` mid-dispatch. Drained
    // in `App::run` AFTER the event handler unwinds so the new WindowState
    // insertion never aliases the outer `windows.borrow()` taken by the
    // dispatch path.
    pub(crate) pending_window_creates: RefCell<Vec<PendingWindowCreate>>,

    // App-level keyboard handler vecs (shared across windows by design).
    pub(super) on_key_down: RefCell<Vec<KeyHandler>>,
    pub(super) on_key_up: RefCell<Vec<KeyHandler>>,
    pub(super) on_text_input: RefCell<Vec<TextInputHandler>>,

    // App-level IME handler vecs.
    pub(super) on_ime_preedit: RefCell<Vec<ImePreeditHandler>>,
    pub(super) on_ime_commit: RefCell<Vec<ImeCommitHandler>>,
    pub(super) on_ime_enabled: RefCell<Vec<ImeLifecycleHandler>>,
    pub(super) on_ime_disabled: RefCell<Vec<ImeLifecycleHandler>>,

    // Native-menu action handlers, keyed by `MenuId`. Populated via
    // `AppContext::on_menu_action`; activation routes through `dispatch_menu`.
    pub(super) menu_registry: RefCell<crate::menu::MenuRegistry>,

    // The active native menu-bar model. Lowered to a `PlatformMenu` and pushed
    // to each window via the platform `set_menu` seam when installed. `None`
    // until an adopter calls `AppContext::set_menu`.
    pub(super) active_menu: RefCell<Option<crate::menu::Menu>>,
}

impl Drop for AppState {
    fn drop(&mut self) {
        log::debug!("AppState dropped — cycle-free shutdown verified");
    }
}
