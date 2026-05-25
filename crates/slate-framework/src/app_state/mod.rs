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

#![allow(dead_code)] // Some methods unused until callers in later layers wire them

use std::rc::Rc;
use std::time::{Duration, Instant};

use slate_platform::{
    Key, KeyCode, Modifiers, PhysicalSize, Platform, Window, WindowId, WindowRenderDelegate,
};
use slate_renderer::{Renderer, RendererObserver};

use crate::app::AppContext;
use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::event::{
    ImeCommitHandler, ImeLifecycleHandler, ImePreeditHandler, KeyHandler, PendingCaptureOp,
    PendingFocusOp, TextInputHandler,
};
use crate::hit_test::HitTestList;
use crate::layout::{compute_layout, resolve_bounds};
use crate::render_cx::RenderCx;
use crate::text_system::TextSystem;
use crate::types::{ElementId, Size};
use crate::view::View;

mod dispatch;
mod guards;
mod lifecycle;
mod state;
mod types;

pub use state::AppState;
pub use types::{AppSignal, DeviceLossReason, RecoveryState};

use guards::{RenderingGuard, SyncResizeGuard, reset_borrow_order};
use types::{
    ADAPTER_PROBE_MIN_INTERVAL_MS, RECOVERY_BACKOFF_BASE_MS, RECOVERY_BACKOFF_STEP_MS,
    RECOVERY_COOLDOWN_MS, RECOVERY_FLAP_GUARD_SECS, RECOVERY_MAX_ATTEMPTS,
};

impl<V: View> AppState<V> {
    /// Apply a queued focus op (drained from an `EventCtx` after the handler
    /// chain unwinds). No-op when `op` is `None`. Per-element key dispatch
    /// also drains through this same apply point after each handler chain.
    pub(crate) fn apply_pending_focus_op(&self, op: Option<PendingFocusOp>) {
        let Some(op) = op else { return };
        let mut reg = self.focus_registry.borrow_mut();
        match op {
            PendingFocusOp::Focus(id) => {
                reg.set_focus(id);
            }
            PendingFocusOp::Blur => reg.clear_focus(),
        }
    }

    /// Apply a queued capture op (drained from an `EventCtx` after the handler
    /// chain unwinds). No-op when `op` is `None`. A `Set` marks the capture
    /// explicit so it survives mouse-up; `Release` clears both the target and
    /// the explicit flag.
    pub(crate) fn apply_pending_capture_op(&self, op: Option<PendingCaptureOp>) {
        let Some(op) = op else { return };
        match op {
            PendingCaptureOp::Set(id) => {
                *self.capture_target.borrow_mut() = Some(id);
                *self.explicit_capture.borrow_mut() = true;
            }
            PendingCaptureOp::Release => {
                *self.capture_target.borrow_mut() = None;
                *self.explicit_capture.borrow_mut() = false;
            }
        }
    }

    /// Clear an active capture if its target produced no hit region this frame
    /// (i.e. the element was unmounted). Called from the prepaint pass after
    /// the hit list is rebuilt.
    fn release_capture_if_unmounted(&self, hit: &HitTestList) {
        let captured = *self.capture_target.borrow();
        if let Some(cap_id) = captured
            && !hit.contains(cap_id)
        {
            *self.capture_target.borrow_mut() = None;
            *self.explicit_capture.borrow_mut() = false;
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
        // skip re-initialization. DO NOT reset recovery_state here â€” that would wipe
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

        // 3. Register cache invalidation observers
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

        // RE-ENTRANCY GUARD â€” applies to BOTH sync and async render paths.
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
        // Gated on `RecoveryState::NotLost` + `!skip_draws` â€” during active
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
                        "adapter LUID mismatch: window={:#018x} renderer={:#018x} â€” marking device-lost",
                        w, a
                    );
                    if let Some(r) = self.renderer.borrow().as_ref() {
                        r.mark_device_potentially_lost();
                    }
                }
            }
        }

        // State machine-driven device-lost recovery
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
                // Classify origin: the renderer's wgpu lost-callback sets a
                // dedicated atomic. If consumed=true the loss came from wgpu;
                // otherwise it came from the per-redraw LUID probe.
                let reason = self.classify_loss_reason();
                let now = Instant::now();

                // Deferral: loss arrived during the modal size/move loop.
                // Park in `DeferredUntilStable` â€” `on_size_move_end` will
                // transition us into `CooldownGate` once the user releases.
                // No render, no retry while deferred.
                if self.window.in_size_move() {
                    log::info!(target: "slate::device_lost",
                        "device-lost during modal size/move loop â€” deferring (reason={:?})", reason);
                    *state = RecoveryState::DeferredUntilStable {
                        detected_at: now,
                        reason,
                    };
                    drop(state);
                    return AppSignal::None;
                }

                // Reason-aware flap guard: only `WgpuCallback` losses count.
                // `LuidMigration` always passes â€” cross-adapter drag is healthy.
                if reason == DeviceLossReason::WgpuCallback {
                    if let Some(prev) = self.last_wgpu_callback_loss_at.get() {
                        let elapsed = now.duration_since(prev);
                        if elapsed <= Duration::from_secs(RECOVERY_FLAP_GUARD_SECS) {
                            log::error!(target: "slate::device_lost",
                                "device-lost re-fired {}ms after prior WgpuCallback (guard={}s, reason=WgpuCallback) â€” giving up",
                                elapsed.as_millis(),
                                RECOVERY_FLAP_GUARD_SECS);
                            *state = RecoveryState::GiveUp { reason };
                            self.last_wgpu_callback_loss_at.set(Some(now));
                            drop(state);
                            return AppSignal::RequestQuit;
                        }
                    }
                    self.last_wgpu_callback_loss_at.set(Some(now));
                }

                log::info!(target: "slate::device_lost",
                    "device loss detected (reason={:?}), entering cooldown", reason);
                *state = RecoveryState::DetectedLost {
                    detected_at: now,
                    reason,
                };
                drop(state);
                self.window.request_redraw();
                return AppSignal::None;
            }
            RecoveryState::DetectedLost {
                detected_at,
                reason,
            } => {
                let reason = self.maybe_upgrade_reason(reason);
                *state = RecoveryState::CooldownGate {
                    since: detected_at,
                    reason,
                };
                drop(state);
                self.window.request_redraw();
                return AppSignal::None;
            }
            RecoveryState::CooldownGate { since, reason } => {
                let reason = self.maybe_upgrade_reason(reason);
                if since.elapsed() < Duration::from_millis(RECOVERY_COOLDOWN_MS) {
                    // Refresh state with possibly-upgraded reason; stay gated.
                    *state = RecoveryState::CooldownGate { since, reason };
                    drop(state);
                    self.window.request_redraw();
                    return AppSignal::None;
                }
                log::info!(target: "slate::device_lost",
                    "cooldown elapsed, starting retry (reason={:?})", reason);
                *state = RecoveryState::Retrying {
                    attempt: 0,
                    last_attempt_at: Instant::now(),
                    reason,
                };
                drop(state);
                return self.execute_recovery_step();
            }
            RecoveryState::Retrying { reason, .. } => {
                let _ = self.maybe_upgrade_reason(reason);
                drop(state);
                return self.execute_recovery_step();
            }
            RecoveryState::DeferredUntilStable { reason, .. } => {
                // Still inside modal size/move loop. Re-apply the reason-
                // upgrade rule so a `WgpuCallback` arriving mid-drag pins
                // the stored reason to the real fault, then skip render.
                // `on_size_move_end` transitions us out into CooldownGate.
                let _ = self.maybe_upgrade_reason(reason);
                drop(state);
                return AppSignal::None;
            }
            RecoveryState::Recovered { .. } => {
                *state = RecoveryState::NotLost;
                drop(state);
                // Fall through to normal redraw
            }
            RecoveryState::GiveUp { .. } => {
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

    /// Classify the origin of a device-loss event by consuming the renderer's
    /// wgpu-callback signal.
    ///
    /// Returns `WgpuCallback` if wgpu's lost-callback fired since the last
    /// consume; otherwise `LuidMigration` (the loss was synthesized by the
    /// per-redraw LUID probe). Must be called on the NotLostâ†’DetectedLost edge.
    fn classify_loss_reason(&self) -> DeviceLossReason {
        let callback_fired = self
            .renderer
            .borrow()
            .as_ref()
            .map(|r| r.consume_wgpu_callback_fired())
            .unwrap_or(false);
        if callback_fired {
            DeviceLossReason::WgpuCallback
        } else {
            DeviceLossReason::LuidMigration
        }
    }

    /// Re-check the wgpu-callback signal during an in-flight recovery cycle
    /// and upgrade the carried reason to `WgpuCallback` if it has fired since
    /// initial classification.
    ///
    /// A `WgpuCallback` event arriving after a `LuidMigration` classification
    /// indicates the cross-monitor drag *also* tripped a real driver fault;
    /// conservative bias counts it. Stamps `last_wgpu_callback_loss_at` so
    /// the next WgpuCallback event observes the spacing correctly.
    fn maybe_upgrade_reason(&self, current: DeviceLossReason) -> DeviceLossReason {
        let callback_fired = self
            .renderer
            .borrow()
            .as_ref()
            .map(|r| r.consume_wgpu_callback_fired())
            .unwrap_or(false);
        if callback_fired && current == DeviceLossReason::LuidMigration {
            self.last_wgpu_callback_loss_at.set(Some(Instant::now()));
            log::info!(target: "slate::device_lost",
                "upgrade-rule: WgpuCallback arrived mid-cycle â€” upgrading reason from LuidMigration");
            DeviceLossReason::WgpuCallback
        } else {
            current
        }
    }

    /// Execute one step of the recovery retry loop.
    ///
    /// Called when `RecoveryState::Retrying`. Handles backoff sleep, renderer
    /// recreation, observer firing, and state transitions.
    fn execute_recovery_step(&self) -> AppSignal {
        let (attempt, reason) = match self.recovery_state.borrow().clone() {
            RecoveryState::Retrying {
                attempt, reason, ..
            } => (attempt, reason),
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
                    return self.handle_recovery_failure(attempt, reason);
                }

                log::info!(target: "slate::device_lost", "GPU device recovered successfully");

                // RT-15: Assign FIRST so observer callbacks that inspect
                // `self.renderer.borrow()` see the new device instead of None.
                // Matches `init_surfaces` ordering (assign â†’ register).
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

                // Cross-monitor recovery just re-picked the adapter for the
                // window's CURRENT monitor. Stamp the probe clock so the
                // 100ms throttle in `dispatch_redraw` doesn't immediately re-probe
                // before the OS-level adapter state has settled.
                self.last_adapter_check_at.set(Some(Instant::now()));

                // Set skip_draws for one-frame present suppression
                self.skip_draws.set(true);

                // Track for flap guard (reason-agnostic, kept for continuity).
                let now = Instant::now();
                self.last_successful_recovery_at.set(Some(now));

                // Discard any late-arriving wgpu-callback signal: this cycle is
                // closed. Without this clear, a callback that fired after
                // classification but before recovery would leak into the next
                // cycle and misclassify a subsequent LuidMigration as
                // WgpuCallback. The atomic is owned by the *current* cycle.
                if let Some(r) = self.renderer.borrow().as_ref() {
                    let leaked = r.consume_wgpu_callback_fired();
                    if leaked {
                        log::trace!(target: "slate::device_lost",
                            "Recovered: cleared late wgpu_callback_fired signal");
                    }
                }

                *self.recovery_state.borrow_mut() = RecoveryState::Recovered { at: now };
                self.window.request_redraw();
                AppSignal::None
            }
            Err(e) => {
                log::error!(target: "slate::device_lost", "GPU device recovery failed: {e}");
                self.handle_recovery_failure(attempt, reason)
            }
        }
    }

    /// Handle a failed recovery attempt.
    fn handle_recovery_failure(&self, attempt: u32, reason: DeviceLossReason) -> AppSignal {
        let next = attempt + 1;
        if next >= RECOVERY_MAX_ATTEMPTS {
            log::error!(target: "slate::device_lost",
                "recovery exhausted after {} attempts (reason={:?})", next, reason);
            *self.recovery_state.borrow_mut() = RecoveryState::GiveUp { reason };
            AppSignal::RequestQuit
        } else {
            *self.recovery_state.borrow_mut() = RecoveryState::Retrying {
                attempt: next,
                last_attempt_at: Instant::now(),
                reason,
            };
            self.window.request_redraw();
            AppSignal::None
        }
    }

    /// Run the redraw pipeline (layout â†’ prepaint â†’ paint â†’ render).
    ///
    /// This is the inner body called by `dispatch_redraw`. The re-entrancy guard
    /// and device-lost recovery wrapper live in `dispatch_redraw`, not here.
    pub(crate) fn run_redraw(&self) {
        // Skip if not initialized
        if self.renderer.borrow().is_none() {
            return;
        }

        // skip_draws gate - suppress one frame after recovery
        if self.skip_draws.get() {
            log::debug!(target: "slate::device_lost", "skip_draws active â€” present suppressed");
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
            let mut render_cx = RenderCx::new(self.window.id());
            slate_reactive::with_observer(self.view_observer_id, || v.render(&mut render_cx))
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
            let mut mhm = self.mouse_handler_map.borrow_mut();
            let mut pm = self.parent_map.borrow_mut();
            let mut khm = self.key_handler_map.borrow_mut();
            let mut fr = self.focus_registry.borrow_mut();
            let mut fb = self.focus_bounds.borrow_mut();
            let mut ihm = self.ime_handler_map.borrow_mut();
            let mut iri = self.ime_registered_ids.borrow_mut();

            hit.clear();
            a11y.clear();
            hm.clear();
            mhm.clear();
            pm.clear();
            khm.clear();
            fr.clear();
            fb.clear();
            ihm.clear();
            iri.clear();
            // Clear the dirty bit; entries are pruned after the walk
            // using `ime_registered_ids` so per-element `Rc<RefCell<ImeState>>`
            // handles survive across frames for surviving elements.
            self.ime_registry.borrow_mut().clear();

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
                &mut mhm,
                &mut pm,
                &mut khm,
                &mut fr,
                &mut fb,
                &self.ime_registry,
                &mut ihm,
                &mut iri,
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

            // Clear focus if the focused element was unmounted this frame.
            fr.prune_missing();
            // Drop IME entries for unmounted elements.
            self.ime_registry.borrow_mut().prune_missing(&iri);

            // Auto-release mouse capture if the captured element was unmounted
            // this frame (no hit region produced). An unmounted element can no
            // longer receive pointer events, so a sticky explicit capture must
            // not strand input on a dead id.
            self.release_capture_if_unmounted(&hit);
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
                &self.ime_registry,
                Some(self.window.as_ref()),
            );

            root.paint(root_bounds, &mut cx);

            // Refresh the IME query cache after every paint so the platform
            // delegate sees the freshly-painted `caret_client_rect`.
            // NLL drops `cx`'s borrows here since it's not used past this point.
            self.republish_ime_cache();

            // Focus ring overlay â€” emitted last so it sits on top of
            // element content. Only painted when the focused element opted into
            // a visible ring via `focus_ring(true)` (default for `focusable`).
            let focused = self.focus_registry.borrow().focused();
            if let Some(id) = focused {
                let registry = self.focus_registry.borrow();
                let show_ring = registry.entry(id).map(|e| e.focus_ring).unwrap_or(false);
                drop(registry);
                if show_ring && let Some(info) = self.focus_bounds.borrow().get(&id).copied() {
                    crate::focus_ring::emit_focus_ring(&mut s, info);
                }
            }
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
    /// same PhysicalSize twice per drag tick (logicalâ†’backing rounding),
    /// and re-running configure on the wgpu surface for the same dimensions
    /// would be wasted GPU work mid-drag.
    pub(crate) fn run_resize_sync(&self, size: PhysicalSize) {
        if self.last_resize_size.get() == Some(size) {
            return;
        }
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.resize(size.as_tuple(), self.window.logical_size());
        }
        self.last_resize_size.set(Some(size));
    }

    /// Event::WindowResized arm â€” currently a no-op.
    /// Platform now drives WindowRedrawRequested post-resize.
    pub fn handle_window_resized(&self, physical_size: (u32, u32)) {
        if let Some(r) = self.renderer.borrow_mut().as_mut() {
            r.resize(physical_size, self.window.logical_size());
        }
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

}

// ---------------------------------------------------------------------------
// WindowRenderDelegate impl â€” sync resize/redraw from platform callbacks
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
        // RT-2.5: must use dispatch_redraw, not run_redraw â€” sync resize
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
        // Proactive device health check on WM_DISPLAYCHANGE / WM_DPICHANGED.
        // Called when monitor topology changes (resolution, monitor plug/unplug, DPI change).
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
                "on_display_change: device probe found loss â†’ requesting redraw");
            self.window.request_redraw();
        }
    }

    fn on_size_move_end(&self, _window_id: WindowId) {
        // Called when modal size-move loop ends (WM_EXITSIZEMOVE).
        // The platform layer already fired any deferred display_change probe.
        // If we deferred a device-lost during the drag, hand it off to
        // `CooldownGate` now so recovery resumes on the next dispatch_redraw
        // tick (keeping `Renderer::new` off this thread's call stack).
        log::trace!(target: "slate::win", "on_size_move_end: modal loop ended");

        let snapshot = self.recovery_state.borrow().clone();
        if let RecoveryState::DeferredUntilStable { reason, .. } = snapshot {
            log::info!(target: "slate::device_lost",
                "exit size/move â€” resuming recovery via cooldown gate (reason={:?})", reason);
            *self.recovery_state.borrow_mut() = RecoveryState::CooldownGate {
                since: Instant::now(),
                reason,
            };
            self.last_adapter_check_at.set(None);
            self.window.request_redraw();
        }
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

    /// Synthetic `LuidMigration`-origin device loss: sets the renderer's
    /// `device_lost` atomic without touching `wgpu_callback_fired`, so the
    /// classifier reports `DeviceLossReason::LuidMigration`.
    ///
    /// Only available with the `test-hooks` feature.
    #[cfg(all(target_os = "windows", feature = "test-hooks"))]
    pub fn force_renderer_device_lost_luid_migration_for_test(&self) -> bool {
        if let Some(r) = self.renderer.borrow().as_ref() {
            r.force_device_lost_luid_migration();
            true
        } else {
            false
        }
    }

    /// Consume the renderer's `wgpu_callback_fired` flag, returning prior value.
    /// Mirrors `Renderer::consume_wgpu_callback_fired` for the cycle-leak
    /// regression test.
    #[cfg(feature = "test-hooks")]
    pub fn consume_wgpu_callback_fired_for_test(&self) -> bool {
        self.renderer
            .borrow()
            .as_ref()
            .map(|r| r.consume_wgpu_callback_fired())
            .unwrap_or(false)
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

    /// Install App-level keyboard handlers. Test-only wrapper exposing the
    /// `pub(crate)` setter so integration tests can wire handlers post-construction.
    pub fn install_key_handlers_for_test(
        &self,
        on_key_down: Vec<KeyHandler>,
        on_key_up: Vec<KeyHandler>,
        on_text_input: Vec<TextInputHandler>,
    ) {
        self.install_key_handlers(on_key_down, on_key_up, on_text_input);
    }

    /// Dispatch a synthetic `KeyDown` to installed handlers. Test-only.
    pub fn dispatch_key_down_for_test(
        &self,
        code: KeyCode,
        key: Key,
        modifiers: Modifiers,
        is_repeat: bool,
    ) -> AppSignal {
        self.dispatch_key_down(code, key, modifiers, is_repeat)
    }

    /// Dispatch a synthetic `KeyUp` to installed handlers. Test-only.
    pub fn dispatch_key_up_for_test(
        &self,
        code: KeyCode,
        key: Key,
        modifiers: Modifiers,
    ) -> AppSignal {
        self.dispatch_key_up(code, key, modifiers)
    }

    /// Dispatch a synthetic `TextInput` to installed handlers. Test-only.
    pub fn dispatch_text_input_for_test(&self, text: String) -> AppSignal {
        self.dispatch_text_input(text)
    }

    /// Dispatch a synthetic `MouseDown` at `position` to installed handlers.
    /// Test-only. Use together with direct writes to `hit_test_list` to
    /// stage hit-test geometry for the synthetic click.
    pub fn dispatch_mouse_down_for_test(
        &self,
        position: (f32, f32),
        button: crate::event::MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
        self.dispatch_mouse_down(position, button, modifiers)
    }

    /// Dispatch a synthetic `MouseMoved` then flush the coalesced move so the
    /// `on_mouse_move` handlers fire on the same call. Test-only.
    pub fn dispatch_mouse_move_for_test(&self, position: (f32, f32)) -> AppSignal {
        let _ = self.dispatch_mouse_moved(position, Modifiers::default());
        self.flush_coalesced_move();
        AppSignal::RequestRedraw
    }

    /// Dispatch a synthetic `MouseMoved` and return its **raw** `AppSignal`
    /// without the masking flush. Test-only â€” lets tests assert whether a bare
    /// move requests a redraw (drag-in-progress) versus stays silent (idle
    /// hover), which `dispatch_mouse_move_for_test` hides by hardcoding
    /// `RequestRedraw`.
    pub fn dispatch_mouse_moved_raw_for_test(&self, position: (f32, f32)) -> AppSignal {
        self.dispatch_mouse_moved(position, Modifiers::default())
    }

    /// Dispatch a synthetic `MouseUp` at `position`. Test-only.
    pub fn dispatch_mouse_up_for_test(
        &self,
        position: (f32, f32),
        button: crate::event::MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
        self.dispatch_mouse_up(position, button, modifiers)
    }

    /// Dispatch a synthetic `CaptureLost`. Test-only.
    pub fn dispatch_capture_lost_for_test(&self) -> AppSignal {
        self.dispatch_capture_lost()
    }

    /// Register a per-element keyboard handler bundle. Test-only â€” production
    /// code wires this through `PrepaintCtx::register_key_handlers` from Div.
    pub fn install_element_key_handlers_for_test(
        &self,
        id: ElementId,
        handlers: crate::event::KeyHandlers,
    ) {
        self.key_handler_map.borrow_mut().insert(id, handlers);
    }

    /// Register a focusable entry directly into the registry. Test-only.
    pub fn register_focusable_for_test(&self, entry: crate::focus::FocusableEntry) {
        self.focus_registry.borrow_mut().register(entry);
    }

    /// Set focus to the given element. Test-only â€” bypasses the focusable check
    /// (mirrors `AppContext::set_focus` minus the redraw request).
    pub fn set_focus_for_test(&self, id: ElementId) {
        self.focus_registry.borrow_mut().set_focus(id);
    }

    /// Insert a parent_map edge for building the focused-chain. Test-only â€”
    /// production code wires this through `PrepaintCtx::register_handlers`.
    pub fn set_parent_for_test(&self, child: ElementId, parent: ElementId) {
        self.parent_map.borrow_mut().insert(child, parent);
    }

    /// Read the current focused element. Test-only.
    pub fn focused_for_test(&self) -> Option<ElementId> {
        self.focus_registry.borrow().focused()
    }

    // -------------------------------------------------------------------------
    // IME test-hook accessors
    // -------------------------------------------------------------------------

    /// Install App-level IME handlers. Test-only wrapper exposing the
    /// `pub(crate)` setter so integration tests can wire handlers
    /// post-construction.
    pub fn install_ime_handlers_for_test(
        &self,
        on_ime_preedit: Vec<ImePreeditHandler>,
        on_ime_commit: Vec<ImeCommitHandler>,
        on_ime_enabled: Vec<ImeLifecycleHandler>,
        on_ime_disabled: Vec<ImeLifecycleHandler>,
    ) {
        self.install_ime_handlers(
            on_ime_preedit,
            on_ime_commit,
            on_ime_enabled,
            on_ime_disabled,
        );
    }

    /// Register a per-element IME handler bundle. Test-only.
    pub fn install_element_ime_handlers_for_test(
        &self,
        id: ElementId,
        handlers: crate::event::ImeHandlers,
    ) {
        self.ime_handler_map.borrow_mut().insert(id, handlers);
    }

    /// Register a per-element mouse handler bundle. Test-only â€” production
    /// code wires this through `PrepaintCtx::register_mouse_handlers` from
    /// TextField.
    pub fn install_element_mouse_handlers_for_test(
        &self,
        id: ElementId,
        handlers: crate::event::MouseHandlers,
    ) {
        self.mouse_handler_map.borrow_mut().insert(id, handlers);
    }

    /// Register an IME-capable element in the registry (get-or-insert).
    /// Returns the shared `Rc<RefCell<ImeState>>`. Test-only.
    pub fn register_ime_state_for_test(
        &self,
        id: ElementId,
    ) -> std::rc::Rc<std::cell::RefCell<crate::ime::ImeState>> {
        self.ime_registry.borrow_mut().register(id)
    }

    /// Write an `ImeState` directly into the registry for `id`, then
    /// republish the cache. Test-only â€” bypasses the platform dispatch path.
    pub fn set_ime_state_for_test(&self, id: ElementId, new_state: crate::ime::ImeState) {
        let rc = self.ime_registry.borrow_mut().register(id);
        *rc.borrow_mut() = new_state;
        self.republish_ime_cache();
    }

    /// Dispatch a synthetic `ImeEnabled` event. Test-only.
    pub fn dispatch_ime_enabled_for_test(&self, window: slate_platform::WindowId) -> AppSignal {
        self.dispatch_ime_enabled(window)
    }

    /// Dispatch a synthetic `ImeDisabled` event. Test-only.
    pub fn dispatch_ime_disabled_for_test(&self, window: slate_platform::WindowId) -> AppSignal {
        self.dispatch_ime_disabled(window)
    }

    /// Dispatch a synthetic `ImePreedit` event. Test-only.
    pub fn dispatch_ime_preedit_for_test(
        &self,
        window: slate_platform::WindowId,
        text: String,
        cursor_byte_offset: usize,
        selection: Option<std::ops::Range<usize>>,
    ) -> AppSignal {
        self.dispatch_ime_preedit(window, text, cursor_byte_offset, selection)
    }

    /// Dispatch a synthetic `ImeCommit` event. Test-only.
    pub fn dispatch_ime_commit_for_test(
        &self,
        window: slate_platform::WindowId,
        text: String,
    ) -> AppSignal {
        self.dispatch_ime_commit(window, text)
    }

    /// Return the `WindowId` of the window backing this `AppState`. Test-only.
    pub fn window_id_for_test(&self) -> slate_platform::WindowId {
        use slate_platform::Window;
        self.window.id()
    }

    /// Run the unmount-driven capture release against the current hit list.
    /// Test-only â€” exercises the same path the prepaint pass uses.
    pub fn release_capture_if_unmounted_for_test(&self) {
        let hit = self.hit_test_list.borrow();
        self.release_capture_if_unmounted(&hit);
    }

    /// Re-publish the IME cache snapshot. Test-only.
    pub fn republish_ime_cache_for_test(&self) {
        self.republish_ime_cache();
    }

    /// Query the cached caret rect via `WindowImeDelegate`. Test-only.
    pub fn ime_caret_rect_query_for_test(&self) -> Option<slate_platform::PhysicalRect> {
        use slate_platform::WindowImeDelegate;
        WindowImeDelegate::ime_caret_rect(self, self.window_id_for_test())
    }

    /// Query a text substring from the cached IME snapshot. Test-only.
    pub fn ime_text_query_for_test(&self, range: std::ops::Range<usize>) -> Option<String> {
        use slate_platform::WindowImeDelegate;
        WindowImeDelegate::ime_text(self, self.window_id_for_test(), range)
    }
}
