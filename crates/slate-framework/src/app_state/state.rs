//! `AppState<V>` struct definition + `Drop` impl.
//!
//! Field visibility note: handler-vec fields (`on_key_down`, `on_text_input`,
//! `on_ime_*`) use `pub(super)` so sibling submodules (`dispatch/key`,
//! `dispatch/text_input`, `dispatch/ime`) can write to them after the file
//! split. The visibility is still private outside `app_state/`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use slate_platform::{DefaultWindow, PhysicalSize};
use slate_reactive::{ObserverId, Signal};
use slate_renderer::{Renderer, Scene};

use crate::event::{
    Handlers, ImeCommitHandler, ImeHandlers, ImeLifecycleHandler, ImePreeditHandler, KeyHandler,
    KeyHandlers, MouseHandlers, TextInputHandler,
};
use crate::executor::{Executor, RedrawRequester};
use crate::focus::FocusRegistry;
use crate::hit_test::HitTestList;
use crate::image_cache::{ImageCache, ImageSystemObserver};
use crate::ime::{CachedImeQuery, ImeRegistry, PendingImeOp};
use crate::layout::LayoutTree;
use crate::paint_cache::{TextShapingCache, TextShapingCacheObserver};
use crate::reactive_state::StateRegistry;
use crate::text_system::{TextSystem, TextSystemObserver};
use crate::types::{AccessibilityNode, ElementId};
use crate::view::View;

use super::types::RecoveryState;

/// Shared application state accessible by both event handler and resize callback.
///
/// Generic over `V: View` to hold the user's root view type.
/// Each field keeps its own `RefCell<T>` wrapper to preserve fine-grained borrow scope.
pub struct AppState<V: View> {
    // Deferred initialization (set in Event::Resumed)
    pub renderer: RefCell<Option<Renderer>>,
    pub text_system: Rc<RefCell<Option<TextSystem>>>,
    pub view: RefCell<Option<V>>,

    // Per-frame state (always allocated)
    pub layout_tree: RefCell<LayoutTree>,
    pub hit_test_list: RefCell<HitTestList>,
    pub a11y_nodes: RefCell<Vec<AccessibilityNode>>,
    pub scene: RefCell<Scene>,

    // Mouse event dispatch state
    pub(crate) handler_map: RefCell<HashMap<ElementId, Handlers>>,
    /// Per-element focused mouse handler bundles. Sister map to `handler_map`
    /// for elements (TextField, future TextArea) that want the focused
    /// three-method bundle. Walked alongside `handler_map` by the same
    /// dispatchers, in the same ancestor order.
    pub(crate) mouse_handler_map: RefCell<HashMap<ElementId, MouseHandlers>>,
    pub parent_map: RefCell<HashMap<ElementId, ElementId>>,
    pub hovered_element: RefCell<Option<ElementId>>,
    pub button_state: RefCell<u8>,
    pub capture_target: RefCell<Option<ElementId>>,
    /// True when the active capture was requested explicitly by a handler
    /// (`EventCtx::set_capture`) rather than auto-set on mouse-down. Explicit
    /// capture is sticky: it is NOT released on mouse-up, so a handler can
    /// drive a multi-step drag across button releases. Cleared by
    /// `release_capture`, by capture-lost, or when the captured element is
    /// dropped from the tree.
    pub explicit_capture: RefCell<bool>,
    pub last_mouse_pos: RefCell<Option<(f32, f32)>>,

    // Move coalescing state
    pub coalesced_move_pos: RefCell<Option<(f32, f32)>>,
    pub last_dispatched_move_pos: RefCell<Option<(f32, f32)>>,

    // Reactive runtime
    pub runtime: Arc<slate_reactive::Runtime>,
    pub view_observer_id: ObserverId,

    // Element-level reactive state
    pub(crate) state_registry: RefCell<StateRegistry>,
    pub text_shaping_cache: Rc<RefCell<TextShapingCache>>,

    // Device-lost cache invalidation observers (held as Rc to keep alive)
    pub text_system_observer: Rc<TextSystemObserver>,
    pub text_shaping_cache_observer: Rc<TextShapingCacheObserver>,

    // Image cache for uploaded images (survives device-lost via observer)
    pub(crate) image_cache: Rc<RefCell<ImageCache>>,
    pub(crate) image_system_observer: Rc<ImageSystemObserver>,

    // Executor (foreground + background)
    pub executor: Executor,
    pub redraw_requester: RedrawRequester,

    // Window reference for size queries
    pub window: Arc<DefaultWindow>,

    // Device-lost recovery state machine
    pub recovery_state: RefCell<RecoveryState>,
    /// One-frame present suppression after recovery.
    pub skip_draws: Cell<bool>,
    /// Post-recovery flap clock. Stamped on every successful recovery
    /// regardless of reason. Retained for log continuity and downstream
    /// consumers; the reason-aware predicate uses `last_wgpu_callback_loss_at`.
    pub last_successful_recovery_at: Cell<Option<Instant>>,
    /// WgpuCallback inter-loss clock. Stamped on each `WgpuCallback`
    /// classification edge. Drives the reason-aware flap guard: 2×
    /// `WgpuCallback` within `RECOVERY_FLAP_GUARD_SECS` → `GiveUp`.
    /// `LuidMigration` losses do not stamp or read this clock.
    pub last_wgpu_callback_loss_at: Cell<Option<Instant>>,
    /// Timestamp of the last adapter-LUID probe. Skip the probe for
    /// `ADAPTER_PROBE_MIN_INTERVAL_MS` after the previous one so a
    /// cross-monitor drag that straddles the seam for multiple frames does
    /// not repeatedly fire `mark_device_lost`.
    pub last_adapter_check_at: Cell<Option<Instant>>,

    // Renderer generation signal: increments on each successful device rebuild.
    // Reactive consumers can subscribe to this to detect device recreation.
    pub renderer_generation: Signal<u64>,

    // Re-entrancy guard: prevents nested render calls from causing RefCell panics
    pub rendering: Cell<bool>,

    // Sync-path quit signal: set by sync delegate methods, read at next event tick
    pub pending_quit: Cell<bool>,

    // Sync-resize flag: set inside on_resize_sync so the redraw it dispatches
    // routes through the renderer's sync-present path (lands the new
    // framebuffer in AppKit's currently open resize CATransaction). Cleared
    // by SyncResizeGuard's Drop. macOS only — Windows ignores it.
    pub sync_resize: Cell<bool>,

    // Idempotency for sync resize: skip work when the new bounds equal the
    // size we already rendered into. AppKit can fire setFrameSize: with the
    // same PhysicalSize twice in a tick (e.g. logical→backing rounding).
    pub last_resize_size: Cell<Option<PhysicalSize>>,

    // App-level keyboard handlers. Per-element key handlers are looked up by
    // ElementId during dispatch; these remain the permanent home for global
    // shortcuts. Vecs preserve registration order; multiple handlers compose.
    pub(super) on_key_down: RefCell<Vec<KeyHandler>>,
    pub(super) on_key_up: RefCell<Vec<KeyHandler>>,
    pub(super) on_text_input: RefCell<Vec<TextInputHandler>>,

    // Per-element keyboard handlers. Looked up by ElementId during
    // the focused-chain bubble dispatch. Mirrors `handler_map` for mouse.
    pub(crate) key_handler_map: RefCell<HashMap<ElementId, KeyHandlers>>,

    // Focus registry. `Rc<RefCell<_>>` so `AppContext` can hold a clone for
    // the outside-handler focus API. Repopulated each prepaint via `clear` +
    // `register` calls; `prune_missing` after the walk auto-clears focus if
    // the previously-focused element was unmounted.
    pub(crate) focus_registry: Rc<RefCell<FocusRegistry>>,

    // Painted bounds of focusable elements (used by the focus ring overlay).
    // Populated during prepaint, consumed once per paint pass. Sparse —
    // only focusable elements contribute entries.
    pub(crate) focus_bounds: RefCell<HashMap<ElementId, crate::focus_ring::FocusBounds>>,

    // -- IME state ------------------------------------------------------------
    /// Per-frame IME state registry. Cleared at the start of every prepaint;
    /// `prune_missing` after the walk drops state for unmounted elements.
    pub(crate) ime_registry: RefCell<ImeRegistry>,
    /// Per-element IME handler bundles. Populated during prepaint.
    pub(crate) ime_handler_map: RefCell<HashMap<ElementId, ImeHandlers>>,
    /// Set of element ids that re-registered with the registry this frame
    /// (mirrors the `clear → re-register → prune_missing` lifecycle used by
    /// `FocusRegistry`).
    pub(crate) ime_registered_ids: RefCell<HashSet<ElementId>>,
    /// Snapshot consumed by [`WindowImeDelegate`] sync queries — republished
    /// at deterministic points (end of `dispatch_ime_preedit`, end of every
    /// paint) so the OS query channel never traverses the live registry.
    pub(crate) cached_ime_query: RefCell<CachedImeQuery>,
    /// Deferred IME ops queued during dispatch (e.g. Tab-during-composition
    /// synthetic commits). Drained by `AppState` AFTER the per-element
    /// handler chain unwinds and BEFORE `pending_focus_op`.
    pub(crate) pending_ime_ops: RefCell<Vec<PendingImeOp>>,

    // App-level IME handlers — observe-only counterparts to the per-element
    // bundle. Each Vec preserves registration order; multiple handlers compose.
    pub(super) on_ime_preedit: RefCell<Vec<ImePreeditHandler>>,
    pub(super) on_ime_commit: RefCell<Vec<ImeCommitHandler>>,
    pub(super) on_ime_enabled: RefCell<Vec<ImeLifecycleHandler>>,
    pub(super) on_ime_disabled: RefCell<Vec<ImeLifecycleHandler>>,
}

impl<V: View> Drop for AppState<V> {
    fn drop(&mut self) {
        log::debug!("AppState<V> dropped — cycle-free shutdown verified");
    }
}
