//! Shared application state for event handler and resize callback.
//!
//! `AppState<V>` holds all RefCell-wrapped state that was previously captured
//! by the `App::run` closure. Wrapping in `Rc<AppState<V>>` allows both the
//! event handler and the sync resize callback to share the same state.
//!
//! # Borrow Order (ADR-001)
//! Fields are borrowed in a fixed order to avoid deadlock:
//! 1. view  2. layout_tree  3. text_system  4. hit_test_list
//! 5. a11y_nodes  6. scene  7. renderer  8. state_registry  9. text_shaping_cache
//!
//! Each borrow is released before the next begins. Do NOT hold multiple borrows
//! simultaneously unless they are proven non-conflicting.

#![allow(dead_code)] // Phase 1: some methods unused until later phases wire them

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slate_platform::{
    DefaultWindow, Modifiers, MouseButton, PhysicalSize, Platform, Window, WindowId,
    WindowRenderDelegate,
};
use slate_reactive::{ObserverId, Signal};
use slate_renderer::{Renderer, RendererObserver, Scene};
use smallvec::SmallVec;

use crate::app::AppContext;
use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::event::{
    EventCtx, Handlers, MouseEvent, MouseHandler, PointerEvent, PointerEventKind, PointerHandler,
    ScrollEvent, ScrollHandler,
};
use crate::executor::{Executor, RedrawRequester};
use crate::hit_test::HitTestList;
use crate::image_cache::{ImageCache, ImageSystemObserver};
use crate::layout::{LayoutTree, compute_layout, resolve_bounds};
use crate::paint_cache::{TextShapingCache, TextShapingCacheObserver};
use crate::reactive_state::StateRegistry;
use crate::text_system::{TextSystem, TextSystemObserver};
use crate::types::{AccessibilityNode, ElementId, Point, Size};
use crate::view::View;

// Recovery state machine constants (Phase 3)
pub(crate) const RECOVERY_COOLDOWN_MS: u64 = 350;
pub(crate) const RECOVERY_MAX_ATTEMPTS: u32 = 5;
pub(crate) const RECOVERY_BACKOFF_BASE_MS: u64 = 100;
pub(crate) const RECOVERY_BACKOFF_STEP_MS: u64 = 10;
pub(crate) const RECOVERY_FLAP_GUARD_SECS: u64 = 5;

// Phase 4 (H3): minimum spacing between adapter-LUID probes in `dispatch_redraw`.
// During a cross-monitor drag the window can cross the boundary many times in
// quick succession; probing every redraw would mark device-lost repeatedly and
// thrash the recovery state machine. 100ms is short enough to feel instant on
// a single deliberate drag yet long enough to absorb the natural drag-jitter
// burst (~60fps × 1–2 frames straddling the seam).
pub(crate) const ADAPTER_PROBE_MIN_INTERVAL_MS: u64 = 100;

/// Recovery state machine for device-lost handling (Phase 3).
///
/// Replaces the old 3-shot immediate retry with a zed-validated pattern:
/// 350ms cooldown, 5-attempt backoff, and skip_draws gating.
#[derive(Debug, Clone)]
pub enum RecoveryState {
    /// Device is healthy, no recovery in progress.
    NotLost,
    /// Device loss just detected; waiting to transition to cooldown.
    DetectedLost { detected_at: Instant },
    /// Waiting for 350ms cooldown before retry attempts.
    CooldownGate { detected_at: Instant },
    /// Actively retrying device recreation.
    Retrying {
        attempt: u32,
        last_attempt_at: Instant,
    },
    /// Recovery succeeded; will transition to NotLost on next redraw.
    Recovered,
    /// Recovery exhausted all attempts; app should quit.
    GiveUp,
}

/// Signal returned by dispatch methods to communicate with the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppSignal {
    None,
    RequestQuit,
    RequestRedraw,
}

// Debug-mode borrow-order discipline (see ADR-001).
// Detects borrow-order violations before they ship.
#[cfg(debug_assertions)]
thread_local! {
    static BORROW_ORDER: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(debug_assertions)]
fn reset_borrow_order() {
    BORROW_ORDER.with(|c| c.set(0));
}

#[cfg(not(debug_assertions))]
fn reset_borrow_order() {}

/// RAII guard for the rendering flag.
struct RenderingGuard<'a>(&'a Cell<bool>);

impl Drop for RenderingGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// RAII guard that holds the sync-resize flag true for the duration of the
/// AppKit selector that opened the resize CATransaction, then clears it
/// unconditionally on Drop so a panic in the resize/dispatch path cannot
/// leave subsequent normal redraws stuck on the sync-present codepath.
struct SyncResizeGuard<'a>(&'a Cell<bool>);

impl<'a> SyncResizeGuard<'a> {
    fn new(flag: &'a Cell<bool>) -> Self {
        flag.set(true);
        Self(flag)
    }
}

impl Drop for SyncResizeGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

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
    pub parent_map: RefCell<HashMap<ElementId, ElementId>>,
    pub hovered_element: RefCell<Option<ElementId>>,
    pub button_state: RefCell<u8>,
    pub capture_target: RefCell<Option<ElementId>>,
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

    // Phase 4: Device-lost cache invalidation observers (held as Rc to keep alive)
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

    // Device-lost recovery state machine (Phase 3)
    pub recovery_state: RefCell<RecoveryState>,
    /// One-frame present suppression after recovery (Phase 3).
    pub skip_draws: Cell<bool>,
    /// 5-second flap guard: if device-lost re-fires within 5s of recovery, give up (Phase 3).
    pub last_successful_recovery_at: Cell<Option<Instant>>,
    /// Phase 4 (H3): timestamp of the last adapter-LUID probe. Skip the probe
    /// for `ADAPTER_PROBE_MIN_INTERVAL_MS` after the previous one so a
    /// cross-monitor drag that straddles the seam for multiple frames does
    /// not repeatedly fire `mark_device_lost`.
    pub last_adapter_check_at: Cell<Option<Instant>>,

    // Renderer generation signal (Phase 1): increments on each successful device rebuild.
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
}

impl<V: View> AppState<V> {
    /// Create a new AppState with uninitialized renderer/text_system/view.
    ///
    /// These are set during `Event::Resumed` via the view factory.
    /// Wires up the reactive runtime's redraw bridge internally.
    pub fn new(
        window: Arc<DefaultWindow>,
        executor: Executor,
        redraw_requester: RedrawRequester,
        runtime: Arc<slate_reactive::Runtime>,
    ) -> Self {
        // Wire the reactive runtime's redraw bridge (moved from app.rs closure)
        runtime.install_redraw({
            let req = redraw_requester.clone();
            Arc::new(move || req.request())
        });

        let view_observer_id = runtime.next_observer_id();
        let state_registry = StateRegistry::new(runtime.clone());

        // Create Rc-wrapped caches for observer weak references (Phase 4)
        let text_system = Rc::new(RefCell::new(None));
        let text_shaping_cache = Rc::new(RefCell::new(TextShapingCache::new()));

        // Create observers with weak references to the caches
        let text_system_observer = Rc::new(TextSystemObserver::new(Rc::downgrade(&text_system)));
        let text_shaping_cache_observer = Rc::new(TextShapingCacheObserver::new(Rc::downgrade(
            &text_shaping_cache,
        )));

        // Image cache + observer (Phase 2)
        let image_cache = Rc::new(RefCell::new(ImageCache::new()));
        let image_system_observer = Rc::new(ImageSystemObserver::new(Rc::downgrade(&image_cache)));

        Self {
            renderer: RefCell::new(None),
            text_system,
            view: RefCell::new(None),

            layout_tree: RefCell::new(LayoutTree::new()),
            hit_test_list: RefCell::new(HitTestList::new()),
            a11y_nodes: RefCell::new(Vec::new()),
            scene: RefCell::new(Scene::new()),

            handler_map: RefCell::new(HashMap::new()),
            parent_map: RefCell::new(HashMap::new()),
            hovered_element: RefCell::new(None),
            button_state: RefCell::new(0),
            capture_target: RefCell::new(None),
            last_mouse_pos: RefCell::new(None),

            coalesced_move_pos: RefCell::new(None),
            last_dispatched_move_pos: RefCell::new(None),

            runtime: runtime.clone(),
            view_observer_id,

            state_registry: RefCell::new(state_registry),
            text_shaping_cache,

            text_system_observer,
            text_shaping_cache_observer,
            image_cache,
            image_system_observer,

            executor,
            redraw_requester,
            window,

            recovery_state: RefCell::new(RecoveryState::NotLost),
            skip_draws: Cell::new(false),
            last_successful_recovery_at: Cell::new(None),
            last_adapter_check_at: Cell::new(None),
            renderer_generation: Signal::new(runtime, 0u64),
            rendering: Cell::new(false),
            pending_quit: Cell::new(false),
            sync_resize: Cell::new(false),
            last_resize_size: Cell::new(None),
        }
    }

    /// Initialize renderer + text_system + view. Called from Event::Resumed.
    /// Re-entry guarded: if renderer is already Some, returns Ok without re-allocating.
    pub fn init_surfaces<P: Platform>(
        &self,
        view_factory: &mut impl FnMut(&AppContext) -> V,
        cx: &AppContext,
        platform: &P,
    ) -> Result<(), String> {
        // Re-entry guard: if already initialized (e.g. screen unlock fires Resumed again),
        // skip re-initialization. DO NOT reset recovery_state here — that would wipe
        // an active recovery counter. (Red-team RT-1.6)
        if self.renderer.borrow().is_some() {
            return Ok(());
        }

        // FIRST INIT path:
        // 1. Build renderer
        let renderer = match pollster::block_on(Renderer::new(self.window.clone())) {
            Ok(r) => r,
            Err(e) => {
                log::error!("renderer init failed: {e}");
                platform.quit();
                return Err(format!("renderer init failed: {e}"));
            }
        };

        // 2. Build text_system
        let text_system = match TextSystem::new() {
            Ok(ts) => ts,
            Err(e) => {
                log::error!("text system init failed: {e}");
                platform.quit();
                return Err(format!("text system init failed: {e}"));
            }
        };

        log::info!("renderer and text system ready");

        // 3. Register cache invalidation observers (Phase 4 + Phase 2)
        renderer.register_observer(
            Rc::downgrade(&self.text_system_observer) as std::rc::Weak<dyn RendererObserver>
        );
        renderer
            .register_observer(Rc::downgrade(&self.text_shaping_cache_observer)
                as std::rc::Weak<dyn RendererObserver>);
        renderer.register_observer(
            Rc::downgrade(&self.image_system_observer) as std::rc::Weak<dyn RendererObserver>
        );

        // 4. Store components + update generation signal
        let renderer_gen = renderer.current_generation();
        *self.renderer.borrow_mut() = Some(renderer);
        *self.text_system.borrow_mut() = Some(text_system);
        *self.view.borrow_mut() = Some(view_factory(cx));
        self.renderer_generation.set(renderer_gen);

        // 5. Reset state (only on first init)
        *self.recovery_state.borrow_mut() = RecoveryState::NotLost;
        self.skip_draws.set(false);
        self.rendering.set(false);
        self.pending_quit.set(false);

        // 5. Request initial redraw
        self.window.request_redraw();

        Ok(())
    }

    /// Full redraw dispatch with device-lost recovery wrapper + re-entrancy guard.
    /// Returns AppSignal::RequestQuit if recovery exceeds RECOVERY_MAX_ATTEMPTS.
    pub fn dispatch_redraw(&self) -> AppSignal {
        // Phase-2-reopen trace: snapshot guard + flag BEFORE re-entrancy gate
        // so we see every entry, even the bailed-on-rendering=true ones.
        let pre_rendering = self.rendering.get();
        let pre_device_lost = self
            .renderer
            .borrow()
            .as_ref()
            .map(|r| r.is_device_lost())
            .unwrap_or(false);
        log::trace!(
            target: "slate::device_lost",
            "dispatch_redraw entry: rendering={pre_rendering} device_lost={pre_device_lost}"
        );

        // RE-ENTRANCY GUARD — applies to BOTH sync and async render paths.
        // If a redraw is already in flight, skip the duplicate.
        if self.rendering.get() {
            return AppSignal::None;
        }
        self.rendering.set(true);
        let _guard = RenderingGuard(&self.rendering);

        reset_borrow_order();

        // Skip if not initialized
        if self.renderer.borrow().is_none() {
            return AppSignal::None;
        }

        // Adapter-LUID probe: detect cross-monitor drag onto a different
        // physical adapter and mark the device lost so the recovery state
        // machine re-picks an adapter matching the window's current monitor.
        //
        // Gated on `RecoveryState::NotLost` + `!skip_draws` — during active
        // recovery the renderer still reports the OLD adapter's LUID, so an
        // unconditional probe would re-mark device-lost on every retry step
        // and could trip the 5-second flap guard.
        //
        // Throttle: the 100ms minimum interval absorbs the multi-frame burst
        // produced by a cross-monitor drag straddling the seam.
        //
        // No-op on non-Windows: `current_monitor_luid` returns `None` from
        // the default trait impl and `current_adapter_luid` returns `None`
        // on non-Dx12 backends.
        {
            let healthy = matches!(*self.recovery_state.borrow(), RecoveryState::NotLost)
                && !self.skip_draws.get();
            let now = Instant::now();
            let recently_probed = self
                .last_adapter_check_at
                .get()
                .map(|t| {
                    now.duration_since(t) < Duration::from_millis(ADAPTER_PROBE_MIN_INTERVAL_MS)
                })
                .unwrap_or(false);
            if healthy && !recently_probed {
                self.last_adapter_check_at.set(Some(now));
                let window_luid = self.window.current_monitor_luid();
                let adapter_luid = self
                    .renderer
                    .borrow()
                    .as_ref()
                    .and_then(|r| r.current_adapter_luid());
                if let (Some(w), Some(a)) = (window_luid, adapter_luid)
                    && w != a
                {
                    log::info!(
                        target: "slate::device_lost",
                        "adapter LUID mismatch: window={:#018x} renderer={:#018x} — marking device-lost",
                        w, a
                    );
                    if let Some(r) = self.renderer.borrow().as_ref() {
                        r.mark_device_potentially_lost();
                    }
                }
            }
        }

        // Phase 3: State machine-driven device-lost recovery
        let device_lost = {
            let r = self.renderer.borrow();
            r.as_ref().map(|r| r.is_device_lost()).unwrap_or(false)
        };

        // Drive the state machine
        let mut state = self.recovery_state.borrow_mut();
        // Phase-2-reopen trace: which arm fires? Crucial for distinguishing
        // "state machine never reached" vs "reached but fell through".
        log::trace!(
            target: "slate::device_lost",
            "dispatch_redraw match: state={:?} device_lost={device_lost}",
            &*state
        );
        match state.clone() {
            RecoveryState::NotLost if device_lost => {
                // 5-second flap guard: if device-lost re-fires within 5s of recovery, give up
                if let Some(t) = self.last_successful_recovery_at.get() {
                    let elapsed = t.elapsed();
                    if elapsed < Duration::from_secs(RECOVERY_FLAP_GUARD_SECS) {
                        log::error!(target: "slate::device_lost",
                            "device-lost re-fired {}ms after recovery (guard={}s) - giving up",
                            elapsed.as_millis(),
                            RECOVERY_FLAP_GUARD_SECS);
                        *state = RecoveryState::GiveUp;
                        drop(state);
                        return AppSignal::RequestQuit;
                    }
                }
                log::info!(target: "slate::device_lost", "device loss detected, entering cooldown");
                *state = RecoveryState::DetectedLost {
                    detected_at: Instant::now(),
                };
                drop(state);
                self.window.request_redraw();
                return AppSignal::None;
            }
            RecoveryState::DetectedLost { detected_at } => {
                *state = RecoveryState::CooldownGate { detected_at };
                drop(state);
                self.window.request_redraw();
                return AppSignal::None;
            }
            RecoveryState::CooldownGate { detected_at } => {
                if detected_at.elapsed() < Duration::from_millis(RECOVERY_COOLDOWN_MS) {
                    drop(state);
                    self.window.request_redraw();
                    return AppSignal::None;
                }
                log::info!(target: "slate::device_lost", "cooldown elapsed, starting retry");
                *state = RecoveryState::Retrying {
                    attempt: 0,
                    last_attempt_at: Instant::now(),
                };
                drop(state);
                return self.execute_recovery_step();
            }
            RecoveryState::Retrying { .. } => {
                drop(state);
                return self.execute_recovery_step();
            }
            RecoveryState::Recovered => {
                *state = RecoveryState::NotLost;
                drop(state);
                // Fall through to normal redraw
            }
            RecoveryState::GiveUp => {
                return AppSignal::RequestQuit;
            }
            RecoveryState::NotLost => {
                drop(state);
                // Fall through to normal redraw
            }
        }

        // Run the actual redraw
        self.run_redraw();

        AppSignal::None
    }

    /// Execute one step of the recovery retry loop (Phase 3).
    ///
    /// Called when `RecoveryState::Retrying`. Handles backoff sleep, renderer
    /// recreation, observer firing, and state transitions.
    fn execute_recovery_step(&self) -> AppSignal {
        let attempt = match self.recovery_state.borrow().clone() {
            RecoveryState::Retrying { attempt, .. } => attempt,
            _ => return AppSignal::None,
        };

        // Backoff sleep (except first attempt)
        if attempt > 0 {
            let backoff = RECOVERY_BACKOFF_BASE_MS + (attempt as u64) * RECOVERY_BACKOFF_STEP_MS;
            log::debug!(target: "slate::device_lost", "recovery backoff sleep: {}ms", backoff);
            std::thread::sleep(Duration::from_millis(backoff));
        }

        log::info!(target: "slate::device_lost",
            "attempting GPU device recovery (attempt {}/{})",
            attempt + 1, RECOVERY_MAX_ATTEMPTS);

        // Atomic drop (constraint #5): release borrow before rebuild
        *self.renderer.borrow_mut() = None;

        // Recreate renderer
        match pollster::block_on(Renderer::new(self.window.clone())) {
            Ok(new_renderer) => {
                // Health-probe: check if the new renderer is already device-lost
                if new_renderer.is_device_lost() {
                    log::warn!(target: "slate::device_lost",
                        "new renderer is already device-lost, treating as failure");
                    return self.handle_recovery_failure(attempt);
                }

                log::info!(target: "slate::device_lost", "GPU device recovered successfully");

                // RT-15: Assign FIRST so observer callbacks that inspect
                // `self.renderer.borrow()` see the new device instead of None.
                // Matches `init_surfaces` ordering (assign → register).
                *self.renderer.borrow_mut() = Some(new_renderer);

                // Register cache-invalidation observers on the now-installed renderer.
                {
                    let r = self.renderer.borrow();
                    let r = r.as_ref().expect("renderer just assigned");
                    r.register_observer(Rc::downgrade(&self.text_system_observer)
                        as std::rc::Weak<dyn RendererObserver>);
                    r.register_observer(Rc::downgrade(&self.text_shaping_cache_observer)
                        as std::rc::Weak<dyn RendererObserver>);
                    r.register_observer(Rc::downgrade(&self.image_system_observer)
                        as std::rc::Weak<dyn RendererObserver>);
                    // Fire only on recovery (not init): caches built against the
                    // dead device must be invalidated before the next paint.
                    r.fire_observers();
                }

                let renderer_gen = self
                    .renderer
                    .borrow()
                    .as_ref()
                    .map(|r| r.current_generation())
                    .unwrap_or(0);
                self.renderer_generation.set(renderer_gen);

                // Phase 4 (H3): cross-monitor recovery just re-picked the adapter
                // for the window's CURRENT monitor. Stamp the probe clock so the
                // 100ms throttle in `dispatch_redraw` doesn't immediately re-probe
                // before the OS-level adapter state has settled.
                self.last_adapter_check_at.set(Some(Instant::now()));

                // Set skip_draws for one-frame present suppression
                self.skip_draws.set(true);

                // Track for flap guard
                self.last_successful_recovery_at.set(Some(Instant::now()));

                *self.recovery_state.borrow_mut() = RecoveryState::Recovered;
                self.window.request_redraw();
                AppSignal::None
            }
            Err(e) => {
                log::error!(target: "slate::device_lost", "GPU device recovery failed: {e}");
                self.handle_recovery_failure(attempt)
            }
        }
    }

    /// Handle a failed recovery attempt.
    fn handle_recovery_failure(&self, attempt: u32) -> AppSignal {
        let next = attempt + 1;
        if next >= RECOVERY_MAX_ATTEMPTS {
            log::error!(target: "slate::device_lost",
                "recovery exhausted after {} attempts", next);
            *self.recovery_state.borrow_mut() = RecoveryState::GiveUp;
            AppSignal::RequestQuit
        } else {
            *self.recovery_state.borrow_mut() = RecoveryState::Retrying {
                attempt: next,
                last_attempt_at: Instant::now(),
            };
            self.window.request_redraw();
            AppSignal::None
        }
    }

    /// Run the redraw pipeline (layout → prepaint → paint → render).
    ///
    /// This is the inner body called by `dispatch_redraw`. The re-entrancy guard
    /// and device-lost recovery wrapper live in `dispatch_redraw`, not here.
    pub(crate) fn run_redraw(&self) {
        // Skip if not initialized
        if self.renderer.borrow().is_none() {
            return;
        }

        // Phase 3: skip_draws gate - suppress one frame after recovery
        if self.skip_draws.get() {
            log::debug!(target: "slate::device_lost", "skip_draws active — present suppressed");
            self.skip_draws.set(false);
            return;
        }

        let (lw, lh) = self.window.logical_size();
        let scale_factor = self.window.scale_factor();

        // Drain reactive effects
        self.runtime.drain_dirty();
        self.runtime.drain_effects();

        // 1. Build element tree
        let mut root = {
            let mut v = self.view.borrow_mut();
            let v = v.as_mut().expect("view not initialized");
            slate_reactive::with_observer(self.view_observer_id, || v.render())
        };

        // 2. Layout pass
        let root_id = {
            let mut tree = self.layout_tree.borrow_mut();
            tree.clear();

            let mut ts = self.text_system.borrow_mut();
            let ts = ts.as_mut().expect("text system not initialized");

            let mut cx = LayoutCtx::new(
                tree.inner_mut(),
                ts,
                &self.executor.foreground,
                scale_factor,
            );

            compute_layout(&mut root, &mut cx, Size::new(lw as f32, lh as f32))
        };

        let Some(root_id) = root_id else {
            log::warn!("layout computation failed");
            return;
        };

        // 3. Resolve root bounds
        let root_bounds = {
            let tree = self.layout_tree.borrow();
            resolve_bounds(tree.inner(), root_id)
        };

        let Some(root_bounds) = root_bounds else {
            log::warn!("bounds resolution failed");
            return;
        };

        // 4. Prepaint pass
        {
            let tree = self.layout_tree.borrow();
            let mut hit = self.hit_test_list.borrow_mut();
            let mut a11y = self.a11y_nodes.borrow_mut();
            let mut ts = self.text_system.borrow_mut();
            let ts = ts.as_mut().expect("text system not initialized");
            let mut sr = self.state_registry.borrow_mut();
            let mut tsc = self.text_shaping_cache.borrow_mut();
            let mut hm = self.handler_map.borrow_mut();
            let mut pm = self.parent_map.borrow_mut();

            hit.clear();
            a11y.clear();
            hm.clear();
            pm.clear();

            let mut cx = PrepaintCtx::new(
                tree.inner(),
                &mut hit,
                &mut a11y,
                ts,
                &self.executor.foreground,
                scale_factor,
                &mut sr,
                &mut tsc,
                &mut hm,
                &mut pm,
            );

            cx.init_root_frame();
            root.prepaint(root_bounds, &mut cx);

            // Verify prepaint frames are balanced
            debug_assert!(
                cx.id_stack.len() == 1,
                "unbalanced prepaint frames: expected 1 (root), got {}",
                cx.id_stack.len()
            );
            debug_assert!(
                cx.a11y_stack.is_empty(),
                "unbalanced a11y stack at frame end: {} unclosed nodes",
                cx.a11y_stack.len()
            );
        }

        // 4a. Coalesced move flush
        self.flush_coalesced_move();

        // 4b. Hover diff
        self.update_hover_state();

        // 5. Paint pass
        {
            let tree = self.layout_tree.borrow();
            let mut s = self.scene.borrow_mut();
            let mut r = self.renderer.borrow_mut();
            let r = r.as_mut().expect("renderer not initialized");
            let mut ts = self.text_system.borrow_mut();
            let ts = ts.as_mut().expect("text system not initialized");

            s.clear();

            let (glyph_atlas, image_atlas, queue) = r.atlases_and_queue();
            let mut ic = self.image_cache.borrow_mut();
            let mut cx = PaintCtx::new(
                tree.inner(),
                &mut s,
                ts,
                glyph_atlas,
                image_atlas,
                &mut ic,
                queue,
                &self.executor.foreground,
                scale_factor,
            );

            root.paint(root_bounds, &mut cx);
        }

        // 6. Render
        {
            let mut s = self.scene.borrow_mut();
            let mut r = self.renderer.borrow_mut();
            let r = r.as_mut().expect("renderer not initialized");

            // On macOS during a sync-resize tick, present inside AppKit's
            // open CATransaction so the new framebuffer lands in the same
            // transaction as the bounds change.
            #[cfg(target_os = "macos")]
            let render_result = if self.sync_resize.get() {
                r.render_scene_sync(&mut s)
            } else {
                r.render_scene(&mut s)
            };
            #[cfg(not(target_os = "macos"))]
            let render_result = r.render_scene(&mut s);

            if let Err(e) = render_result {
                log::warn!("render skipped: {e:?}");
            }
        }

        // 7. Poll async executor
        self.executor.foreground.poll();

        // 8. GC stale state slots
        {
            let mut sr = self.state_registry.borrow_mut();
            sr.advance_frame();
            sr.gc();
        }

        // 9. GC text shaping cache
        {
            let mut tsc = self.text_shaping_cache.borrow_mut();
            tsc.advance_frame();
            tsc.gc();
        }
    }

    /// Run synchronous resize: resize the renderer.
    /// Caller is responsible for triggering redraw (sync path calls dispatch_redraw after).
    ///
    /// Idempotent: skips work when the requested size matches the last
    /// size we already configured. AppKit can fire setFrameSize: with the
    /// same PhysicalSize twice per drag tick (logical→backing rounding),
    /// and re-running configure on the wgpu surface for the same dimensions
    /// would be wasted GPU work mid-drag.
    pub(crate) fn run_resize_sync(&self, size: PhysicalSize) {
        if self.last_resize_size.get() == Some(size) {
            return;
        }
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.resize(size.as_tuple());
        }
        self.last_resize_size.set(Some(size));
    }

    /// Event::WindowResized arm — currently a no-op.
    /// Platform now drives WindowRedrawRequested post-resize.
    pub fn handle_window_resized(&self, physical_size: (u32, u32)) {
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.resize(physical_size);
        }
    }

    /// Handle background task completion (Event::Wake).
    pub fn handle_wake(&self) -> AppSignal {
        self.executor.foreground.poll();
        AppSignal::RequestRedraw
    }

    /// Handle window close by platform (Event::WindowDestroyed).
    /// Cleans up view and logs. Idempotent.
    pub fn handle_window_destroyed(&self) -> AppSignal {
        log::debug!("WindowDestroyed received in AppState");
        *self.view.borrow_mut() = None;
        AppSignal::RequestQuit
    }

    /// Handle device-lost event from platform.
    pub(crate) fn dispatch_device_lost(&self, fatal: bool) -> AppSignal {
        if fatal {
            log::error!("GPU device lost (fatal) - recovery failed after max attempts");
            AppSignal::RequestQuit
        } else {
            log::warn!("GPU device lost - recovery will be attempted");
            AppSignal::None
        }
    }

    /// Handle device-restored event from platform.
    pub(crate) fn dispatch_device_restored(&self) -> AppSignal {
        log::info!("GPU device restored - rendering resumed");
        *self.recovery_state.borrow_mut() = RecoveryState::NotLost;
        AppSignal::RequestRedraw
    }

    // -----------------------------------------------------------------------
    // Mouse event dispatch methods (Phase 2)
    // -----------------------------------------------------------------------

    /// Dispatch MouseDown event.
    pub(crate) fn dispatch_mouse_down(
        &self,
        position: (f32, f32),
        button: MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
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
        let old_state = *self.button_state.borrow();
        *self.button_state.borrow_mut() = old_state | bit;

        // Determine dispatch target
        let captured = *self.capture_target.borrow();
        let target = if let Some(ct) = captured {
            Some(ct)
        } else {
            let hit = self
                .hit_test_list
                .borrow()
                .hit_test(Point::new(position.0, position.1));
            if let Some(result) = hit {
                *self.capture_target.borrow_mut() = Some(result.element_id);
                Some(result.element_id)
            } else {
                None
            }
        };

        if let Some(t) = target {
            // Collect handlers first, then invoke (clone-before-drop pattern)
            let mouse_handlers: SmallVec<[MouseHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_mouse_down.clone()))
                    .collect()
            };
            let pointer_handlers: SmallVec<[PointerHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_event.clone()))
                    .collect()
            };

            // Invoke handlers (borrows released)
            let mut stopped = false;
            for handler in &mouse_handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }

            stopped = false;
            for handler in &pointer_handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&pointer_event, &mut ctx);
                if stopped {
                    break;
                }
            }
        }

        *self.last_mouse_pos.borrow_mut() = Some(position);
        AppSignal::RequestRedraw
    }

    /// Dispatch MouseUp event.
    pub(crate) fn dispatch_mouse_up(
        &self,
        position: (f32, f32),
        button: MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
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
        let old_state = *self.button_state.borrow();
        let new_state = old_state & !bit;
        *self.button_state.borrow_mut() = new_state;

        // Determine dispatch target
        let captured = *self.capture_target.borrow();
        let up_hit = self
            .hit_test_list
            .borrow()
            .hit_test(Point::new(position.0, position.1))
            .map(|r| r.element_id);
        let target = captured.or(up_hit);

        if let Some(t) = target {
            // Collect handlers (clone-before-drop pattern)
            let mouse_up_handlers: SmallVec<[MouseHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_mouse_up.clone()))
                    .collect()
            };
            let pointer_handlers: SmallVec<[PointerHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_event.clone()))
                    .collect()
            };
            let click_handlers: SmallVec<[MouseHandler; 8]> =
                if button == MouseButton::Left && up_hit == captured && captured.is_some() {
                    let hm = self.handler_map.borrow();
                    let pm = self.parent_map.borrow();
                    ancestors(t, &pm)
                        .filter_map(|id| hm.get(&id).and_then(|h| h.on_click.clone()))
                        .collect()
                } else {
                    SmallVec::new()
                };

            // Invoke handlers (borrows released)
            let mut stopped = false;
            for handler in &mouse_up_handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }

            stopped = false;
            for handler in &pointer_handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&pointer_event, &mut ctx);
                if stopped {
                    break;
                }
            }

            stopped = false;
            for handler in &click_handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }
        }

        // Release capture when all buttons are up
        if new_state == 0 {
            *self.capture_target.borrow_mut() = None;
        }

        *self.last_mouse_pos.borrow_mut() = Some(position);
        AppSignal::RequestRedraw
    }

    /// Dispatch MouseMoved event.
    pub(crate) fn dispatch_mouse_moved(
        &self,
        position: (f32, f32),
        modifiers: Modifiers,
    ) -> AppSignal {
        let pointer_event = PointerEvent {
            kind: PointerEventKind::Move,
            position,
            button: None,
            modifiers,
            timestamp: Instant::now(),
        };

        // Route through capture_target if captured, else hit-test
        let captured = *self.capture_target.borrow();
        let target = if let Some(ct) = captured {
            Some(ct)
        } else {
            self.hit_test_list
                .borrow()
                .hit_test(Point::new(position.0, position.1))
                .map(|r| r.element_id)
        };

        if let Some(t) = target {
            // Collect handlers (clone-before-drop pattern)
            let handlers: SmallVec<[PointerHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_event.clone()))
                    .collect()
            };

            // Invoke handlers
            let mut stopped = false;
            for handler in &handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&pointer_event, &mut ctx);
                if stopped {
                    break;
                }
            }
        }

        *self.coalesced_move_pos.borrow_mut() = Some(position);
        *self.last_mouse_pos.borrow_mut() = Some(position);
        AppSignal::None
    }

    /// Dispatch MouseScrolled event.
    pub(crate) fn dispatch_mouse_scrolled(
        &self,
        position: (f32, f32),
        delta_x: f32,
        delta_y: f32,
        precise: bool,
        modifiers: Modifiers,
    ) -> AppSignal {
        let scroll_event = ScrollEvent {
            position,
            delta_x,
            delta_y,
            precise,
            modifiers,
            timestamp: Instant::now(),
        };

        let hit = self
            .hit_test_list
            .borrow()
            .hit_test(Point::new(position.0, position.1));

        if let Some(result) = hit {
            // Collect handlers (clone-before-drop pattern)
            let handlers: SmallVec<[ScrollHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(result.element_id, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_mouse_scrolled.clone()))
                    .collect()
            };

            // Invoke handlers
            let mut stopped = false;
            for handler in &handlers {
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&scroll_event, &mut ctx);
                if stopped {
                    break;
                }
            }
        }

        AppSignal::RequestRedraw
    }

    /// Dispatch MouseExited event.
    pub(crate) fn dispatch_mouse_exited(&self) -> AppSignal {
        let old_hover = *self.hovered_element.borrow();
        if old_hover.is_some() {
            // Collect handlers (clone-before-drop pattern)
            let handlers: SmallVec<[PointerHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                if let Some(id) = old_hover {
                    ancestors(id, &pm)
                        .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_leave.clone()))
                        .collect()
                } else {
                    SmallVec::new()
                }
            };

            // Invoke handlers
            for handler in &handlers {
                let event = PointerEvent {
                    kind: PointerEventKind::Leave,
                    position: (0.0, 0.0),
                    button: None,
                    modifiers: Modifiers::default(),
                    timestamp: Instant::now(),
                };
                let mut stopped = false;
                let mut ctx = EventCtx::new(&mut stopped);
                handler(&event, &mut ctx);
            }

            *self.hovered_element.borrow_mut() = None;
        }
        *self.last_mouse_pos.borrow_mut() = None;
        *self.coalesced_move_pos.borrow_mut() = None;
        AppSignal::None
    }

    /// Dispatch CaptureLost event.
    pub(crate) fn dispatch_capture_lost(&self) -> AppSignal {
        *self.capture_target.borrow_mut() = None;
        *self.button_state.borrow_mut() = 0;
        AppSignal::None
    }

    // -----------------------------------------------------------------------
    // Mouse event helpers (internal)
    // -----------------------------------------------------------------------

    fn flush_coalesced_move(&self) {
        if let Some(pos) = self.coalesced_move_pos.borrow_mut().take() {
            let last_dispatched = *self.last_dispatched_move_pos.borrow();
            if last_dispatched != Some(pos) {
                let captured = *self.capture_target.borrow();
                let target = if let Some(ct) = captured {
                    Some(ct)
                } else {
                    self.hit_test_list
                        .borrow()
                        .hit_test(Point::new(pos.0, pos.1))
                        .map(|r| r.element_id)
                };

                if let Some(t) = target {
                    let mouse_event = MouseEvent {
                        position: pos,
                        button: None,
                        modifiers: Modifiers::default(),
                        timestamp: Instant::now(),
                    };
                    bubble_mouse_handler(
                        t,
                        &mouse_event,
                        &self.handler_map.borrow(),
                        &self.parent_map.borrow(),
                        |h| h.on_mouse_move.clone(),
                    );
                }

                *self.last_dispatched_move_pos.borrow_mut() = Some(pos);
            }
        }
    }

    fn update_hover_state(&self) {
        let current_pos = *self.last_mouse_pos.borrow();
        let captured = *self.capture_target.borrow();

        let new_hover = if captured.is_some() {
            captured
        } else if let Some(pos) = current_pos {
            self.hit_test_list
                .borrow()
                .hit_test(Point::new(pos.0, pos.1))
                .map(|r| r.element_id)
        } else {
            None
        };

        let old_hover = *self.hovered_element.borrow();
        if new_hover != old_hover {
            fire_hover_transitions(
                old_hover,
                new_hover,
                &self.handler_map.borrow(),
                &self.parent_map.borrow(),
            );
            *self.hovered_element.borrow_mut() = new_hover;
        }
    }
}

impl<V: View> Drop for AppState<V> {
    fn drop(&mut self) {
        log::debug!("AppState<V> dropped — cycle-free shutdown verified");
    }
}

// ---------------------------------------------------------------------------
// WindowRenderDelegate impl — sync resize/redraw from platform callbacks
// ---------------------------------------------------------------------------

impl<V: View> WindowRenderDelegate for AppState<V> {
    fn on_resize_sync(&self, _window_id: WindowId, new_size: PhysicalSize) {
        // Single-window today: window_id ignored.

        // Hold the sync-resize flag for both the resize and the dispatched
        // redraw so the renderer routes the present through CATransaction::flush()
        // and lands the new framebuffer in AppKit's open resize transaction.
        // RAII guard clears the flag on Drop even if a panic unwinds through here.
        let _sync_guard = SyncResizeGuard::new(&self.sync_resize);

        // Step 1: resize the swap chain (cheap, in-place).
        self.run_resize_sync(new_size);

        // Step 2: full redraw THROUGH the recovery wrapper.
        // RT-2.5: must use dispatch_redraw, not run_redraw — sync resize
        // can still hit device-lost (e.g., GPU reset under heavy load), and
        // bypassing the wrapper would either render garbage or panic.
        if self.dispatch_redraw() == AppSignal::RequestQuit {
            self.pending_quit.set(true);
        }
    }

    fn on_redraw(&self, _window_id: WindowId) {
        // Same routing as on_resize_sync's redraw step:
        // dispatch_redraw includes the rendering: Cell<bool> guard and the
        // device-lost recovery wrapper. Raw run_redraw is forbidden here.
        if self.dispatch_redraw() == AppSignal::RequestQuit {
            self.pending_quit.set(true);
        }
    }

    fn on_display_change(&self, _window_id: WindowId) {
        // Phase 5: Proactive device health check on WM_DISPLAYCHANGE / WM_DPICHANGED.
        // Called when monitor topology changes (resolution, monitor plug/unplug, DPI change).
        // Phase 2.1 T3: confirm entry + post-probe verdict.
        log::trace!(target: "slate::device_lost", "on_display_change ENTRY");
        let lost = {
            let r = self.renderer.borrow();
            r.as_ref()
                .map(|r| r.mark_device_potentially_lost())
                .unwrap_or(false)
        };
        log::trace!(target: "slate::device_lost", "on_display_change: probe returned lost={lost}");
        if lost {
            log::info!(target: "slate::device_lost",
                "on_display_change: device probe found loss → requesting redraw");
            self.window.request_redraw();
        }
    }

    fn on_size_move_end(&self, _window_id: WindowId) {
        // Phase 5: Called when modal size-move loop ends (WM_EXITSIZEMOVE).
        // The platform layer already fired any deferred display_change probe,
        // so this is just a hook for future use. Currently no-op beyond logging.
        log::trace!(target: "slate::win", "on_size_move_end: modal loop ended");
    }
}

// ---------------------------------------------------------------------------
// Event dispatch helpers (pub(crate) for use by app.rs and future phases)
// ---------------------------------------------------------------------------

/// Walk ancestors from `start` to root, yielding each ElementId.
pub(crate) fn ancestors<'a>(
    start: ElementId,
    parent_map: &'a HashMap<ElementId, ElementId>,
) -> impl Iterator<Item = ElementId> + 'a {
    let mut current = Some(start);
    std::iter::from_fn(move || {
        let id = current?;
        current = parent_map.get(&id).copied();
        Some(id)
    })
}

/// Bubble a mouse event through the ancestor chain, invoking handlers.
pub(crate) fn bubble_mouse_handler<F>(
    target: ElementId,
    event: &MouseEvent,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
    get_handler: F,
) where
    F: Fn(&Handlers) -> Option<MouseHandler>,
{
    let mut chain: SmallVec<[MouseHandler; 8]> = SmallVec::new();
    for id in ancestors(target, parent_map) {
        if let Some(h) = handler_map
            .get(&id)
            .and_then(|handlers| get_handler(handlers))
        {
            chain.push(h);
        }
    }

    let mut stopped = false;
    for handler in &chain {
        let mut ctx = EventCtx::new(&mut stopped);
        handler(event, &mut ctx);
        if stopped {
            break;
        }
    }
}

/// Bubble a pointer event through the ancestor chain, invoking handlers.
pub(crate) fn bubble_pointer_handler<F>(
    target: ElementId,
    event: &PointerEvent,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
    get_handler: F,
) where
    F: Fn(&Handlers) -> Option<PointerHandler>,
{
    let mut chain: SmallVec<[PointerHandler; 8]> = SmallVec::new();
    for id in ancestors(target, parent_map) {
        if let Some(h) = handler_map
            .get(&id)
            .and_then(|handlers| get_handler(handlers))
        {
            chain.push(h);
        }
    }

    let mut stopped = false;
    for handler in &chain {
        let mut ctx = EventCtx::new(&mut stopped);
        handler(event, &mut ctx);
        if stopped {
            break;
        }
    }
}

/// Bubble a scroll event through the ancestor chain, invoking handlers.
pub(crate) fn bubble_scroll_handler(
    target: ElementId,
    event: &ScrollEvent,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
) {
    let mut chain: SmallVec<[ScrollHandler; 8]> = SmallVec::new();
    for id in ancestors(target, parent_map) {
        if let Some(h) = handler_map
            .get(&id)
            .and_then(|handlers| handlers.on_mouse_scrolled.clone())
        {
            chain.push(h);
        }
    }

    let mut stopped = false;
    for handler in &chain {
        let mut ctx = EventCtx::new(&mut stopped);
        handler(event, &mut ctx);
        if stopped {
            break;
        }
    }
}

/// Fire hover transitions between old and new hover targets.
pub(crate) fn fire_hover_transitions(
    old_hover: Option<ElementId>,
    new_hover: Option<ElementId>,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
) {
    use std::collections::HashSet;

    let old_chain: SmallVec<[ElementId; 16]> = if let Some(id) = old_hover {
        ancestors(id, parent_map).collect()
    } else {
        SmallVec::new()
    };
    let new_chain: SmallVec<[ElementId; 16]> = if let Some(id) = new_hover {
        ancestors(id, parent_map).collect()
    } else {
        SmallVec::new()
    };

    let new_set: HashSet<ElementId> = new_chain.iter().copied().collect();
    let old_set: HashSet<ElementId> = old_chain.iter().copied().collect();

    let mut leave_handlers: SmallVec<[PointerHandler; 8]> = SmallVec::new();
    for &id in &old_chain {
        if !new_set.contains(&id)
            && let Some(h) = handler_map
                .get(&id)
                .and_then(|h| h.on_pointer_leave.clone())
        {
            leave_handlers.push(h);
        }
    }

    let mut enter_ids: SmallVec<[ElementId; 8]> = SmallVec::new();
    for &id in &new_chain {
        if !old_set.contains(&id) {
            enter_ids.push(id);
        }
    }
    enter_ids.reverse();

    let mut enter_handlers: SmallVec<[PointerHandler; 8]> = SmallVec::new();
    for &id in &enter_ids {
        if let Some(h) = handler_map
            .get(&id)
            .and_then(|h| h.on_pointer_enter.clone())
        {
            enter_handlers.push(h);
        }
    }

    for handler in &leave_handlers {
        let event = PointerEvent {
            kind: PointerEventKind::Leave,
            position: (0.0, 0.0),
            button: None,
            modifiers: Modifiers::default(),
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut ctx = EventCtx::new(&mut stopped);
        handler(&event, &mut ctx);
    }

    for handler in &enter_handlers {
        let event = PointerEvent {
            kind: PointerEventKind::Enter,
            position: (0.0, 0.0),
            button: None,
            modifiers: Modifiers::default(),
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut ctx = EventCtx::new(&mut stopped);
        handler(&event, &mut ctx);
    }
}

/// Convert MouseButton to bitmask bit position.
pub(crate) fn button_to_bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1 << 0,
        MouseButton::Right => 1 << 1,
        MouseButton::Middle => 1 << 2,
        MouseButton::Other(n) => 1 << (3 + n.min(4)),
    }
}

// =============================================================================
// Test-only accessors (gated on `test-hooks` feature)
// =============================================================================

#[cfg(any(test, feature = "test-hooks"))]
impl<V: View> AppState<V> {
    /// Current renderer generation. `None` if renderer not yet initialized.
    pub fn renderer_generation(&self) -> Option<u64> {
        self.renderer
            .borrow()
            .as_ref()
            .map(|r| r.current_generation())
    }

    /// True if the renderer reports the device as lost.
    pub fn renderer_is_device_lost(&self) -> bool {
        self.renderer
            .borrow()
            .as_ref()
            .map(|r| r.is_device_lost())
            .unwrap_or(false)
    }

    /// Snapshot of the current recovery state.
    pub fn current_recovery_state(&self) -> RecoveryState {
        self.recovery_state.borrow().clone()
    }

    /// Trigger a synthetic device-lost via `ID3D12Device5::RemoveDevice()`.
    /// Returns `false` if no renderer is initialized.
    ///
    /// Only available with the `test-hooks` feature enabled on both slate-framework
    /// and slate-renderer (the feature propagates via Cargo.toml `test-hooks = ["slate-renderer/test-hooks"]`).
    #[cfg(all(target_os = "windows", feature = "test-hooks"))]
    pub fn force_renderer_device_lost(&self) -> bool {
        if let Some(r) = self.renderer.borrow().as_ref() {
            r.force_device_lost();
            true
        } else {
            false
        }
    }

    /// Fire the device-lost callback logic for testing.
    ///
    /// Mirrors `Renderer::fire_device_lost_callback_for_test`: filters `Destroyed`
    /// (returns false), otherwise captures telemetry and sets atomic flag.
    /// Use to validate the Destroyed filter and state-machine engagement paths
    /// without triggering a real wgpu device-lost condition.
    ///
    /// Only available with the `test-hooks` feature.
    #[cfg(feature = "test-hooks")]
    pub fn fire_renderer_device_lost_callback(
        &self,
        reason: wgpu::DeviceLostReason,
        message: String,
    ) -> bool {
        if let Some(r) = self.renderer.borrow().as_ref() {
            r.fire_device_lost_callback_for_test(reason, message)
        } else {
            false
        }
    }

    /// Read the sync-path quit signal.
    pub fn pending_quit(&self) -> bool {
        self.pending_quit.get()
    }

    /// Request a redraw on the associated window.
    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }
}
