//! Device-lost recovery helpers driven by `dispatch_redraw`'s state machine.
//!
//! - `classify_loss_reason` and `maybe_upgrade_reason` resolve the origin of
//!   a loss event (wgpu callback vs LUID migration probe).
//! - `execute_recovery_step` and `handle_recovery_failure` advance the retry
//!   loop, recreate the renderer, and fire observers.

use std::rc::Rc;
use std::time::Instant;

use slate_platform::{Window, WindowId};
use slate_renderer::{Renderer, RendererObserver};

use super::super::state::AppState;
use super::super::types::{AppSignal, DeviceLossReason, RecoveryState};
use super::recovery_policy::recovery_should_give_up;

impl AppState {
    /// Classify the origin of a device-loss event by consuming the renderer's
    /// wgpu-callback signal for the given window.
    ///
    /// Returns `WgpuCallback` if wgpu's lost-callback fired since last consume;
    /// otherwise `LuidMigration`. Must be called on the `NotLost → DetectedLost`
    /// edge.
    pub(super) fn classify_loss_reason(&self, window_id: WindowId) -> DeviceLossReason {
        let callback_fired = {
            let guard = self.windows.borrow();
            guard
                .get(&window_id)
                .and_then(|win| {
                    win.renderer
                        .borrow()
                        .as_ref()
                        .map(|r| r.consume_wgpu_callback_fired())
                })
                .unwrap_or(false)
        };
        if callback_fired {
            DeviceLossReason::WgpuCallback
        } else {
            DeviceLossReason::LuidMigration
        }
    }

    /// Re-check the wgpu-callback signal during an in-flight recovery cycle
    /// and upgrade the carried reason to `WgpuCallback` if it has fired.
    ///
    /// A `WgpuCallback` arriving after a `LuidMigration` classification means
    /// the cross-monitor drag also tripped a real driver fault; conservative
    /// bias counts it. Stamps `last_wgpu_callback_loss_at` so the next event
    /// observes the spacing correctly.
    pub(super) fn maybe_upgrade_reason(
        &self,
        window_id: WindowId,
        current: DeviceLossReason,
    ) -> DeviceLossReason {
        let callback_fired = {
            let guard = self.windows.borrow();
            guard
                .get(&window_id)
                .and_then(|win| {
                    win.renderer
                        .borrow()
                        .as_ref()
                        .map(|r| r.consume_wgpu_callback_fired())
                })
                .unwrap_or(false)
        };
        if callback_fired && current == DeviceLossReason::LuidMigration {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                win.last_wgpu_callback_loss_at.set(Some(Instant::now()));
            }
            log::info!(target: "slate::device_lost",
                "upgrade-rule: WgpuCallback arrived mid-cycle — upgrading from LuidMigration");
            DeviceLossReason::WgpuCallback
        } else {
            current
        }
    }

    /// Escalate a confirmed cross-adapter mismatch to the recovery machine.
    ///
    /// When the window's current monitor is served by a different GPU than the
    /// renderer was built on (`current_monitor_luid != current_adapter_luid`),
    /// the renderer must rebuild on the window's adapter. Marks the device lost
    /// **unconditionally** and returns `true`.
    ///
    /// Why unconditional: a healthy device on the *wrong* adapter still returns
    /// `S_OK` from `GetDeviceRemovedReason`, so the health-gated
    /// `mark_device_potentially_lost()` vetoes the escalation — the renderer
    /// then keeps cross-adapter DirectComposition-composing until DXGI
    /// hard-removes the device (`0x887A0005`) and the process dies with no
    /// recovery. The confirmed LUID mismatch is itself proof the renderer must
    /// migrate. The loss classifies as `LuidMigration` (the wgpu callback did
    /// not fire), which bypasses the flap guard — correct for a user-driven
    /// monitor move.
    ///
    /// Because `LuidMigration` never self-terminates, adapter selection MUST
    /// converge: the rebuilt renderer has to land on an adapter whose LUID
    /// matches the window's monitor. If `pick_adapter_for_window` cannot honor
    /// that monitor (its adapter is unenumerable / software-filtered) it falls
    /// back to the high-performance GPU, the mismatch persists, and this probe
    /// re-escalates every cooldown — a spaced rebuild loop, not a tight spin.
    /// That degraded state is strictly better than the prior silent
    /// device-removed crash, but a repeated-migration circuit-breaker (off
    /// `last_successful_recovery_at`) is the follow-up if it ever surfaces.
    pub(super) fn probe_adapter_mismatch_and_escalate(&self, window_id: WindowId) -> bool {
        let guard = self.windows.borrow();
        let Some(win) = guard.get(&window_id) else {
            return false;
        };
        let window_luid = win.window.current_monitor_luid();
        let adapter_luid = win
            .renderer
            .borrow()
            .as_ref()
            .and_then(|r| r.current_adapter_luid());
        if let (Some(w), Some(a)) = (window_luid, adapter_luid)
            && w != a
        {
            log::info!(
                target: "slate::device_lost",
                "adapter LUID mismatch window={:?}: window={:#018x} renderer={:#018x} \
                 — forcing device-lost so recovery rebuilds on the window's adapter",
                window_id, w, a
            );
            if let Some(r) = win.renderer.borrow().as_ref() {
                r.mark_device_lost();
            }
            return true;
        }
        false
    }

    /// Execute one step of the recovery retry loop for the given window.
    ///
    /// Called when `RecoveryState::Retrying`. Handles backoff, renderer
    /// recreation, observer firing, and state transitions.
    pub(super) fn execute_recovery_step(&self, window_id: WindowId) -> AppSignal {
        let (attempt, first_failure_at, reason) = {
            let guard = self.windows.borrow();
            match guard
                .get(&window_id)
                .map(|w| w.recovery_state.borrow().clone())
            {
                Some(RecoveryState::Retrying {
                    attempt,
                    first_failure_at,
                    reason,
                    ..
                }) => (attempt, first_failure_at, reason),
                _ => return AppSignal::None,
            }
        };

        // Spacing between attempts is enforced by the caller (`dispatch_redraw`'s
        // `Retrying` arm gates on `recovery_ready_for_next_attempt`), so this
        // never blocks the thread on a backoff sleep — the event loop keeps
        // pumping while the adapter is suspended.
        log::info!(target: "slate::device_lost",
            "attempting GPU device recovery (probe attempt {}, {:?} into this loss)",
            attempt + 1, first_failure_at.elapsed());

        // Get platform window handle.
        let platform_window = {
            let guard = self.windows.borrow();
            guard.get(&window_id).map(|w| w.window.clone())
        };
        let Some(platform_window) = platform_window else {
            return AppSignal::None; // Window was destroyed during recovery.
        };

        // Atomic drop: release old renderer borrow before rebuild.
        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                *win.renderer.borrow_mut() = None;
            }
        }

        // Recreate renderer.
        match pollster::block_on(Renderer::new(platform_window)) {
            Ok(new_renderer) => {
                // Health-probe: check if the new renderer is already device-lost.
                if new_renderer.is_device_lost() {
                    log::warn!(target: "slate::device_lost",
                        "new renderer is already device-lost, treating as failure");
                    return self.handle_recovery_failure(
                        window_id,
                        attempt,
                        first_failure_at,
                        reason,
                    );
                }

                log::info!(target: "slate::device_lost", "GPU device recovered successfully");

                // Assign FIRST so observer callbacks that inspect the renderer see
                // the new device instead of None. Matches init_surfaces ordering.
                {
                    let guard = self.windows.borrow();
                    if let Some(win) = guard.get(&window_id) {
                        *win.renderer.borrow_mut() = Some(new_renderer);
                    }
                }

                // Register the shared text-shaping cache observer on the
                // now-installed renderer and clear this window's per-window
                // caches inline (their atlas was destroyed with the old
                // renderer; entries reference dead AllocIds).
                {
                    let guard = self.windows.borrow();
                    if let Some(win) = guard.get(&window_id) {
                        win.glyph_cache.borrow_mut().clear_cpu_state();
                        win.image_cache.borrow_mut().clear_allocations();

                        let r = win.renderer.borrow();
                        let r = r.as_ref().expect("renderer just assigned");
                        r.register_observer(Rc::downgrade(&self.text_shaping_cache_observer)
                            as std::rc::Weak<dyn RendererObserver>);
                        // Fire only on recovery: caches built against the dead device
                        // must be invalidated before the next paint.
                        r.fire_observers();
                    }
                }

                let renderer_gen = {
                    let guard = self.windows.borrow();
                    guard
                        .get(&window_id)
                        .and_then(|win| {
                            win.renderer
                                .borrow()
                                .as_ref()
                                .map(|r| r.current_generation())
                        })
                        .unwrap_or(0)
                };

                let now = Instant::now();
                {
                    let guard = self.windows.borrow();
                    if let Some(win) = guard.get(&window_id) {
                        win.renderer_generation.set(renderer_gen);
                        // Stamp probe clock: new adapter is now correct for current monitor.
                        win.last_adapter_check_at.set(Some(now));
                        // Suppress one frame after recovery.
                        win.skip_draws.set(true);
                        // Track recovery time for continuity (reason-agnostic).
                        win.last_successful_recovery_at.set(Some(now));

                        // Discard any late-arriving wgpu-callback signal so it doesn't
                        // leak into the next recovery cycle and misclassify a subsequent
                        // LuidMigration as WgpuCallback.
                        if let Some(r) = win.renderer.borrow().as_ref() {
                            let leaked = r.consume_wgpu_callback_fired();
                            if leaked {
                                log::trace!(target: "slate::device_lost",
                                    "Recovered: cleared late wgpu_callback_fired signal");
                            }
                        }

                        *win.recovery_state.borrow_mut() = RecoveryState::Recovered { at: now };
                        win.window.request_redraw();
                    }
                }

                AppSignal::None
            }
            Err(e) => {
                log::error!(target: "slate::device_lost", "GPU device recovery failed: {e}");
                self.handle_recovery_failure(window_id, attempt, first_failure_at, reason)
            }
        }
    }

    /// Handle a failed recovery attempt for the given window.
    ///
    /// Wait-until-healthy policy: keep probing on a spaced schedule until the
    /// adapter comes back. The ONLY quit path is the wall-clock give-up budget
    /// (`recovery_should_give_up`), measured from `first_failure_at` — a
    /// last-resort exit after continuous, unbroken failure, not an
    /// attempt-count reflex.
    pub(super) fn handle_recovery_failure(
        &self,
        window_id: WindowId,
        attempt: u32,
        first_failure_at: Instant,
        reason: DeviceLossReason,
    ) -> AppSignal {
        let elapsed = first_failure_at.elapsed();
        if recovery_should_give_up(elapsed) {
            log::error!(target: "slate::device_lost",
                "device unrecoverable after {:?} of continuous failure ({} probe attempts, reason={:?}) — last-resort quit",
                elapsed, attempt + 1, reason);
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                *win.recovery_state.borrow_mut() = RecoveryState::GiveUp { reason };
            }
            AppSignal::RequestQuit
        } else {
            log::debug!(target: "slate::device_lost",
                "recovery probe {} failed after {:?}; still waiting for a healthy adapter (reason={:?})",
                attempt + 1, elapsed, reason);
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                *win.recovery_state.borrow_mut() = RecoveryState::Retrying {
                    attempt: attempt + 1,
                    last_attempt_at: Instant::now(),
                    first_failure_at,
                    reason,
                };
                win.window.request_redraw();
            }
            AppSignal::None
        }
    }
}
