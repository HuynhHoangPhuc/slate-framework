//! Per-window state owned by `AppState.windows: HashMap<WindowId, WindowState>`.
//!
//! Holds every resource that is naturally bound to a single window: the
//! platform window handle, the GPU renderer, the per-window view, the layout
//! tree, the hit-test list, focus + IME registries, per-element handler maps,
//! per-window pointer state, and the device-lost recovery state machine.
//!
//! Process-wide resources (reactive runtime, executor, text system, image
//! cache, app-level handler vecs) remain on the slimmed [`AppState`].
//!
//! ## Borrow discipline
//!
//! Every outer dispatch entry takes one immutable `self.windows.borrow()`,
//! resolves the routed `WindowId` to a `&WindowState`, then `borrow_mut()`s
//! the specific interior `RefCell`s it needs. Mutations to the HashMap
//! itself (window create / destroy) flow through a deferred-ops queue so
//! a callback can request a new window without aliasing the outer borrow.
//!
//! [`AppState`]: crate::app_state::AppState

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use slate_platform::{DefaultWindow, PhysicalSize, Window, WindowId};
use slate_reactive::{ObserverId, Signal};
use slate_renderer::{Renderer, Scene};
use slate_text::GlyphCache;

use crate::erased_view::ErasedView;
use crate::event::{Handlers, ImeHandlers, KeyHandlers, MouseHandlers};
use crate::elements::overlay::OverlayRegistry;
use crate::focus::FocusRegistry;
use crate::focus_ring::FocusBounds;
use crate::hit_test::HitTestList;
use crate::image_cache::ImageCache;
use crate::ime::{CachedImeQuery, ImeRegistry, PendingImeOp};
use crate::layout::LayoutTree;
use crate::types::{AccessibilityNode, ElementId};

use super::types::RecoveryState;

/// Per-window state. One instance per live platform window, keyed by
/// [`WindowId`] in `AppState.windows`. Field grouping mirrors the scout report
/// (`docs/multi-window-scout.md §4.1`).
pub struct WindowState {
    // -- Identity + platform handle --------------------------------------
    pub id: WindowId,
    /// Concrete platform window. `Window` trait abstraction is post-v1.
    pub window: Arc<DefaultWindow>,

    // -- Deferred init (filled by Event::Resumed) ------------------------
    pub renderer: RefCell<Option<Renderer>>,
    pub view: RefCell<Option<Box<dyn ErasedView>>>,

    // -- Per-window atlas-scoped caches ---------------------------------
    /// Glyph cache paired with this window's `Renderer.glyph_atlas`. Keeping
    /// it per-window prevents cross-window `AllocId` collisions (would
    /// surface as text corruption when multiple windows paint into separate
    /// atlases). CPU-side state is cleared inline by the recovery path when
    /// this window's renderer is rebuilt.
    pub(crate) glyph_cache: RefCell<GlyphCache>,
    /// Image cache paired with this window's `Renderer.image_atlas`. Same
    /// per-atlas invariant as `glyph_cache`. `clear_allocations` is called
    /// on device-lost to drop stale atlas handles while keeping CPU pixels.
    pub(crate) image_cache: RefCell<ImageCache>,

    // -- Per-frame state -------------------------------------------------
    pub layout_tree: RefCell<LayoutTree>,
    pub hit_test_list: RefCell<HitTestList>,
    pub a11y_nodes: RefCell<Vec<AccessibilityNode>>,
    /// macOS VoiceOver adapter, lazily built on the first frame after the
    /// surface is realized (needs a live NSView). `None` on non-AppKit/test
    /// windows. macOS-only — the P7 platform-accessibility seed.
    #[cfg(target_os = "macos")]
    pub(crate) a11y_adapter: RefCell<Option<crate::a11y_macos::MacA11yAdapter>>,
    /// Real window key state, driving the a11y adapter's `update_view_focus_state`
    /// (without which AccessKit reports no focused element). Set on
    /// `Event::WindowFocused`. Defaults to `true` so a single-window app — which
    /// may not see a `WindowFocused` before its first paint — behaves like the
    /// VoiceOver-validated spike; flipped `false` when another window becomes key.
    #[cfg(target_os = "macos")]
    pub(crate) is_key: std::cell::Cell<bool>,
    /// Windows Narrator/UIA adapter, lazily built on the first frame after the
    /// surface is realized and installed as the window's a11y delegate. `None`
    /// on non-Win32/test windows. Windows-only — mirror of `a11y_adapter`.
    #[cfg(target_os = "windows")]
    pub(crate) a11y_adapter: RefCell<Option<std::rc::Rc<crate::a11y_windows::WinA11yAdapter>>>,
    pub scene: RefCell<Scene>,

    // -- Mouse dispatch state -------------------------------------------
    pub(crate) handler_map: RefCell<HashMap<ElementId, Handlers>>,
    /// Sister bundle to `handler_map` for focused-aware mouse handlers
    /// (TextField, future TextArea). Walked alongside the same ancestor chain.
    pub(crate) mouse_handler_map: RefCell<HashMap<ElementId, MouseHandlers>>,
    pub parent_map: RefCell<HashMap<ElementId, ElementId>>,
    pub hovered_element: RefCell<Option<ElementId>>,
    pub button_state: RefCell<u8>,
    pub capture_target: RefCell<Option<ElementId>>,
    /// True when capture was requested explicitly via `EventCtx::set_capture`
    /// (sticky across mouse-up). Cleared by release_capture / capture-lost /
    /// element drop.
    pub explicit_capture: RefCell<bool>,
    pub last_mouse_pos: RefCell<Option<(f32, f32)>>,

    // -- Move coalescing ------------------------------------------------
    pub coalesced_move_pos: RefCell<Option<(f32, f32)>>,
    pub last_dispatched_move_pos: RefCell<Option<(f32, f32)>>,

    // -- Per-window reactive observer -----------------------------------
    pub view_observer_id: ObserverId,

    // -- Per-element keyboard handlers ----------------------------------
    pub(crate) key_handler_map: RefCell<HashMap<ElementId, KeyHandlers>>,

    // -- Focus -----------------------------------------------------------
    /// `Rc` so the outside-handler `AppContext::set_focus(window, id)` path
    /// can hold a clone and operate on the same registry the per-window
    /// dispatch walks against.
    pub(crate) focus_registry: Rc<RefCell<FocusRegistry>>,
    pub(crate) focus_bounds: RefCell<HashMap<ElementId, FocusBounds>>,
    /// Open modal overlays as `(modal_id, focus_to_restore)`, in open order.
    /// Reconciled each frame after the prepaint walk to auto-focus into newly
    /// opened modals and restore prior focus when one closes.
    pub(crate) modal_focus_stack: RefCell<Vec<(ElementId, Option<ElementId>)>>,

    // -- Overlays --------------------------------------------------------
    /// Open overlays registered during the prepaint walk, consulted by the
    /// key/mouse dispatch path for Esc / outside-click dismissal. Rebuilt each
    /// frame (cleared before prepaint); holds the dismiss callbacks.
    pub(crate) overlay_registry: RefCell<OverlayRegistry>,

    // -- IME -------------------------------------------------------------
    /// Wrapped in `Rc` so dispatch loops can clone a reference out before
    /// dropping the outer `windows` borrow (mirrors `focus_registry`).
    pub(crate) ime_registry: Rc<RefCell<ImeRegistry>>,
    pub(crate) ime_handler_map: RefCell<HashMap<ElementId, ImeHandlers>>,
    pub(crate) ime_registered_ids: RefCell<HashSet<ElementId>>,
    /// Snapshot consumed by `WindowImeDelegate` sync queries — republished at
    /// deterministic points so the OS query channel never traverses the live
    /// registry.
    pub(crate) cached_ime_query: RefCell<CachedImeQuery>,
    /// Ops deferred during dispatch (e.g. Tab-during-composition synthetic
    /// commits). Drained AFTER the per-element handler chain unwinds and
    /// BEFORE pending_focus_op.
    pub(crate) pending_ime_ops: RefCell<Vec<PendingImeOp>>,

    // -- Device-lost recovery (per-window flap-guard) -------------------
    pub recovery_state: RefCell<RecoveryState>,
    /// One-frame present suppression after recovery.
    pub skip_draws: Cell<bool>,
    /// Post-recovery flap clock. Stamped on every successful recovery.
    pub last_successful_recovery_at: Cell<Option<Instant>>,
    /// WgpuCallback inter-loss clock. Reason-aware flap guard reads this
    /// (LuidMigration losses bypass).
    pub last_wgpu_callback_loss_at: Cell<Option<Instant>>,
    /// Adapter-LUID probe throttle clock.
    pub last_adapter_check_at: Cell<Option<Instant>>,

    /// Per-window rebuild signal: increments on each successful device
    /// rebuild. Reactive consumers can subscribe.
    pub renderer_generation: Signal<u64>,
    /// Re-entrancy guard for the render path.
    pub rendering: Cell<bool>,

    // -- AppKit sync-resize coordination (macOS only; ignored elsewhere) -
    /// Set inside on_resize_sync so the dispatched redraw routes through the
    /// renderer's sync-present path. Cleared by `SyncResizeGuard::Drop`.
    pub sync_resize: Cell<bool>,
    /// Idempotency for sync resize: skip when bounds match the last paint.
    /// AppKit can fire setFrameSize: with the same size twice in a tick.
    pub last_resize_size: Cell<Option<PhysicalSize>>,
}

impl WindowState {
    /// Construct an empty `WindowState` for the given platform window.
    ///
    /// Renderer / view stay `None` until `Event::Resumed` runs the view
    /// factory and initialises the GPU stack via `init_surfaces`.
    pub fn new(window: Arc<DefaultWindow>, runtime: Arc<slate_reactive::Runtime>) -> Self {
        let id = window.id();
        let view_observer_id = runtime.next_observer_id();
        Self {
            id,
            window,
            renderer: RefCell::new(None),
            view: RefCell::new(None),
            glyph_cache: RefCell::new(GlyphCache::new()),
            image_cache: RefCell::new(ImageCache::new()),
            layout_tree: RefCell::new(LayoutTree::new()),
            hit_test_list: RefCell::new(HitTestList::new()),
            a11y_nodes: RefCell::new(Vec::new()),
            #[cfg(target_os = "macos")]
            a11y_adapter: RefCell::new(None),
            #[cfg(target_os = "macos")]
            is_key: std::cell::Cell::new(true),
            #[cfg(target_os = "windows")]
            a11y_adapter: RefCell::new(None),
            scene: RefCell::new(Scene::new()),
            handler_map: RefCell::new(HashMap::new()),
            mouse_handler_map: RefCell::new(HashMap::new()),
            parent_map: RefCell::new(HashMap::new()),
            hovered_element: RefCell::new(None),
            button_state: RefCell::new(0),
            capture_target: RefCell::new(None),
            explicit_capture: RefCell::new(false),
            last_mouse_pos: RefCell::new(None),
            coalesced_move_pos: RefCell::new(None),
            last_dispatched_move_pos: RefCell::new(None),
            view_observer_id,
            key_handler_map: RefCell::new(HashMap::new()),
            focus_registry: Rc::new(RefCell::new(FocusRegistry::new())),
            focus_bounds: RefCell::new(HashMap::new()),
            modal_focus_stack: RefCell::new(Vec::new()),
            overlay_registry: RefCell::new(OverlayRegistry::default()),
            ime_registry: Rc::new(RefCell::new(ImeRegistry::new())),
            ime_handler_map: RefCell::new(HashMap::new()),
            ime_registered_ids: RefCell::new(HashSet::new()),
            cached_ime_query: RefCell::new(CachedImeQuery::default()),
            pending_ime_ops: RefCell::new(Vec::new()),
            recovery_state: RefCell::new(RecoveryState::NotLost),
            skip_draws: Cell::new(false),
            last_successful_recovery_at: Cell::new(None),
            last_wgpu_callback_loss_at: Cell::new(None),
            last_adapter_check_at: Cell::new(None),
            renderer_generation: Signal::new(runtime, 0u64),
            rendering: Cell::new(false),
            sync_resize: Cell::new(false),
            last_resize_size: Cell::new(None),
        }
    }
}
