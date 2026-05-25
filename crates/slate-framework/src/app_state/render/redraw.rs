//! Redraw pipeline and surface lifecycle.
//!
//! - `init_surfaces` builds the renderer + text system on `Event::Resumed`.
//! - `dispatch_redraw` is the re-entrancy-guarded entry that drives the
//!   device-lost recovery state machine before delegating to `run_redraw`.
//! - `run_redraw` is the inner layout → prepaint → paint → render pass.
//! - `run_resize_sync` / `handle_window_resized` push new dimensions to the
//!   swap chain; `dispatch_device_lost` / `dispatch_device_restored` are the
//!   platform event arms.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slate_platform::{PhysicalSize, Platform, Window};
use slate_renderer::{Renderer, RendererObserver};

use crate::app::AppContext;
use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::layout::{compute_layout, resolve_bounds};
use crate::render_cx::RenderCx;
use crate::text_system::TextSystem;
use crate::types::Size;
use crate::view::View;

use super::super::guards::{RenderingGuard, reset_borrow_order};
use super::super::state::AppState;
use super::super::types::{
    ADAPTER_PROBE_MIN_INTERVAL_MS, AppSignal, DeviceLossReason, RECOVERY_COOLDOWN_MS,
    RECOVERY_FLAP_GUARD_SECS, RecoveryState,
};

impl<V: View> AppState<V> {
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
                // Park in `DeferredUntilStable` — `on_size_move_end` will
                // transition us into `CooldownGate` once the user releases.
                // No render, no retry while deferred.
                if self.window.in_size_move() {
                    log::info!(target: "slate::device_lost",
                        "device-lost during modal size/move loop — deferring (reason={:?})", reason);
                    *state = RecoveryState::DeferredUntilStable {
                        detected_at: now,
                        reason,
                    };
                    drop(state);
                    return AppSignal::None;
                }

                // Reason-aware flap guard: only `WgpuCallback` losses count.
                // `LuidMigration` always passes — cross-adapter drag is healthy.
                if reason == DeviceLossReason::WgpuCallback {
                    if let Some(prev) = self.last_wgpu_callback_loss_at.get() {
                        let elapsed = now.duration_since(prev);
                        if elapsed <= Duration::from_secs(RECOVERY_FLAP_GUARD_SECS) {
                            log::error!(target: "slate::device_lost",
                                "device-lost re-fired {}ms after prior WgpuCallback (guard={}s, reason=WgpuCallback) — giving up",
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

    /// Run the redraw pipeline (layout → prepaint → paint → render).
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

            // Focus ring overlay — emitted last so it sits on top of
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
    /// same PhysicalSize twice per drag tick (logical→backing rounding),
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

    /// Event::WindowResized arm — currently a no-op.
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
