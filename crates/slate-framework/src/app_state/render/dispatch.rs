//! `dispatch_redraw`: the re-entrancy-guarded redraw entry that drives the
//! device-lost recovery state machine before delegating to `run_redraw`.
//!
//! Holds the adapter-LUID probe and the `RecoveryState` match arms; recovery
//! retry mechanics live in `recovery.rs`, the actual pipeline in `redraw.rs`.

use std::time::{Duration, Instant};

use slate_platform::Window;

use crate::view::View;

use super::super::guards::{RenderingGuard, reset_borrow_order};
use super::super::state::AppState;
use super::super::types::{
    ADAPTER_PROBE_MIN_INTERVAL_MS, AppSignal, DeviceLossReason, RECOVERY_COOLDOWN_MS,
    RECOVERY_FLAP_GUARD_SECS, RecoveryState,
};

impl<V: View> AppState<V> {
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
}
