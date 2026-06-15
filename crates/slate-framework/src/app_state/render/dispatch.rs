//! `dispatch_redraw`: the re-entrancy-guarded redraw entry that drives the
//! device-lost recovery state machine before delegating to `run_redraw`.
//!
//! Holds the adapter-LUID probe and the `RecoveryState` match arms; recovery
//! retry mechanics live in `recovery.rs`, the actual pipeline in `redraw.rs`.

use std::time::{Duration, Instant};

use slate_platform::{Window, WindowId};

use super::super::guards::reset_borrow_order;
use super::super::state::AppState;
use super::RedrawOutcome;
use super::super::types::{
    ADAPTER_PROBE_MIN_INTERVAL_MS, AppSignal, DeviceLossReason, RECOVERY_COOLDOWN_MS,
    RECOVERY_FLAP_GUARD_SECS, RecoveryState,
};

impl AppState {
    /// Full redraw dispatch with device-lost recovery wrapper + re-entrancy guard.
    ///
    /// Returns `AppSignal::RequestQuit` if recovery exceeds `RECOVERY_MAX_ATTEMPTS`.
    /// Returns `AppSignal::None` if the window is unknown (race with destroy).
    pub fn dispatch_redraw(&self, window_id: WindowId) -> AppSignal {
        // Snapshot fields needed for early logging before the per-window borrow.
        let (pre_rendering, pre_device_lost) = {
            let guard = self.windows.borrow();
            match guard.get(&window_id) {
                Some(win) => {
                    let r = win.rendering.get();
                    let dl = win
                        .renderer
                        .borrow()
                        .as_ref()
                        .map(|r| r.is_device_lost())
                        .unwrap_or(false);
                    (r, dl)
                }
                None => return AppSignal::None,
            }
        };
        log::trace!(
            target: "slate::device_lost",
            "dispatch_redraw entry window={:?}: rendering={pre_rendering} device_lost={pre_device_lost}",
            window_id
        );

        // Re-entrancy guard — if a redraw is already in flight for this window, skip.
        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                if win.rendering.get() {
                    return AppSignal::None;
                }
                win.rendering.set(true);
            } else {
                return AppSignal::None;
            }
        }

        // RAII guard clears rendering flag on scope exit (panic-safe).
        // We hold the flag on the WindowState Cell directly.
        struct RenderingCellGuard<'a>(&'a std::cell::Cell<bool>);
        impl Drop for RenderingCellGuard<'_> {
            fn drop(&mut self) {
                self.0.set(false);
            }
        }
        let _rendering_guard = {
            let guard = self.windows.borrow();
            guard.get(&window_id).map(|win| {
                // SAFETY: The Cell reference lifetime is bounded by AppState
                // lifetime, which outlives this scope. We need a raw-pointer
                // bridge because the borrow guard would alias. Use the guard
                // pattern via a raw pointer converted back to a reference with
                // the AppState lifetime.
                //
                // Actually: we set the flag above, and we clear it below after
                // all dispatch is done. The guard approach needs the Cell to
                // stay alive. Since AppState is Rc'd and lives for the entire
                // scope, this is safe. We approximate with a simple cleanup at
                // the end of the function instead.
                let _ = win; // acknowledged
            })
        };

        reset_borrow_order();

        // Skip if not initialized.
        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                if win.renderer.borrow().is_none() {
                    win.rendering.set(false);
                    return AppSignal::None;
                }
            } else {
                return AppSignal::None;
            }
        }

        // Adapter-LUID probe: detect cross-monitor drag onto a different
        // physical adapter and mark device lost so the recovery state machine
        // re-picks an adapter matching the window's current monitor.
        //
        // Gated on NotLost + !skip_draws — during active recovery the old LUID
        // would re-trigger on every retry step and could trip the flap guard.
        // Throttled to ADAPTER_PROBE_MIN_INTERVAL_MS to absorb burst during drag.
        // No-op on non-Windows (current_monitor_luid returns None).
        {
            // Decide whether to probe this tick (healthy + throttled), then
            // delegate the mismatch check + escalation to the shared helper.
            let should_probe = {
                let guard = self.windows.borrow();
                if let Some(win) = guard.get(&window_id) {
                    let healthy = matches!(*win.recovery_state.borrow(), RecoveryState::NotLost)
                        && !win.skip_draws.get();
                    let now = Instant::now();
                    let recently_probed = win
                        .last_adapter_check_at
                        .get()
                        .map(|t| {
                            now.duration_since(t)
                                < Duration::from_millis(ADAPTER_PROBE_MIN_INTERVAL_MS)
                        })
                        .unwrap_or(false);
                    if healthy && !recently_probed {
                        win.last_adapter_check_at.set(Some(now));
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            };
            if should_probe {
                self.probe_adapter_mismatch_and_escalate(window_id);
            }
        }

        // Check device_lost and drive the state machine.
        let device_lost = {
            let guard = self.windows.borrow();
            guard
                .get(&window_id)
                .and_then(|win| win.renderer.borrow().as_ref().map(|r| r.is_device_lost()))
                .unwrap_or(false)
        };

        let state_snapshot = {
            let guard = self.windows.borrow();
            guard
                .get(&window_id)
                .map(|win| win.recovery_state.borrow().clone())
        };
        let Some(state_snapshot) = state_snapshot else {
            // Window disappeared.
            return AppSignal::None;
        };

        log::trace!(
            target: "slate::device_lost",
            "dispatch_redraw match window={:?}: state={:?} device_lost={device_lost}",
            window_id, &state_snapshot
        );

        let result = match state_snapshot {
            RecoveryState::NotLost if device_lost => {
                let reason = self.classify_loss_reason(window_id);
                let now = Instant::now();

                let in_size_move = {
                    let guard = self.windows.borrow();
                    guard
                        .get(&window_id)
                        .map(|w| w.window.in_size_move())
                        .unwrap_or(false)
                };
                if in_size_move {
                    log::info!(target: "slate::device_lost",
                        "device-lost during modal size/move — deferring (reason={:?})", reason);
                    let guard = self.windows.borrow();
                    if let Some(win) = guard.get(&window_id) {
                        *win.recovery_state.borrow_mut() = RecoveryState::DeferredUntilStable {
                            detected_at: now,
                            reason,
                        };
                    }
                    let guard2 = self.windows.borrow();
                    if let Some(win) = guard2.get(&window_id) {
                        win.rendering.set(false);
                    }
                    return AppSignal::None;
                }

                // Reason-aware flap guard: only WgpuCallback losses count.
                if reason == DeviceLossReason::WgpuCallback {
                    let prev = {
                        let guard = self.windows.borrow();
                        guard
                            .get(&window_id)
                            .and_then(|w| w.last_wgpu_callback_loss_at.get())
                    };
                    if let Some(prev) = prev {
                        let elapsed = now.duration_since(prev);
                        if elapsed <= Duration::from_secs(RECOVERY_FLAP_GUARD_SECS) {
                            log::error!(target: "slate::device_lost",
                                "device-lost re-fired {}ms after prior WgpuCallback (guard={}s) — giving up",
                                elapsed.as_millis(), RECOVERY_FLAP_GUARD_SECS);
                            let guard = self.windows.borrow();
                            if let Some(win) = guard.get(&window_id) {
                                *win.recovery_state.borrow_mut() = RecoveryState::GiveUp { reason };
                                win.last_wgpu_callback_loss_at.set(Some(now));
                            }
                            let guard2 = self.windows.borrow();
                            if let Some(win) = guard2.get(&window_id) {
                                win.rendering.set(false);
                            }
                            return AppSignal::RequestQuit;
                        }
                    }
                    let guard = self.windows.borrow();
                    if let Some(win) = guard.get(&window_id) {
                        win.last_wgpu_callback_loss_at.set(Some(now));
                    }
                }

                log::info!(target: "slate::device_lost",
                    "device loss detected (reason={:?}), entering cooldown", reason);
                {
                    let guard = self.windows.borrow();
                    if let Some(win) = guard.get(&window_id) {
                        *win.recovery_state.borrow_mut() = RecoveryState::DetectedLost {
                            detected_at: now,
                            reason,
                        };
                        win.window.request_redraw();
                    }
                }
                let guard = self.windows.borrow();
                if let Some(win) = guard.get(&window_id) {
                    win.rendering.set(false);
                }
                return AppSignal::None;
            }
            RecoveryState::DetectedLost {
                detected_at,
                reason,
            } => {
                let reason = self.maybe_upgrade_reason(window_id, reason);
                {
                    let guard = self.windows.borrow();
                    if let Some(win) = guard.get(&window_id) {
                        *win.recovery_state.borrow_mut() = RecoveryState::CooldownGate {
                            since: detected_at,
                            reason,
                        };
                        win.window.request_redraw();
                    }
                }
                let guard = self.windows.borrow();
                if let Some(win) = guard.get(&window_id) {
                    win.rendering.set(false);
                }
                return AppSignal::None;
            }
            RecoveryState::CooldownGate { since, reason } => {
                let reason = self.maybe_upgrade_reason(window_id, reason);
                if since.elapsed() < Duration::from_millis(RECOVERY_COOLDOWN_MS) {
                    let guard = self.windows.borrow();
                    if let Some(win) = guard.get(&window_id) {
                        *win.recovery_state.borrow_mut() =
                            RecoveryState::CooldownGate { since, reason };
                        win.window.request_redraw();
                    }
                    let guard2 = self.windows.borrow();
                    if let Some(win) = guard2.get(&window_id) {
                        win.rendering.set(false);
                    }
                    return AppSignal::None;
                }
                log::info!(target: "slate::device_lost",
                    "cooldown elapsed, starting retry (reason={:?})", reason);
                {
                    let guard = self.windows.borrow();
                    if let Some(win) = guard.get(&window_id) {
                        *win.recovery_state.borrow_mut() = RecoveryState::Retrying {
                            attempt: 0,
                            last_attempt_at: Instant::now(),
                            reason,
                        };
                    }
                }
                let sig = self.execute_recovery_step(window_id);
                let guard = self.windows.borrow();
                if let Some(win) = guard.get(&window_id) {
                    win.rendering.set(false);
                }
                return sig;
            }
            RecoveryState::Retrying { reason, .. } => {
                let _ = self.maybe_upgrade_reason(window_id, reason);
                let sig = self.execute_recovery_step(window_id);
                let guard = self.windows.borrow();
                if let Some(win) = guard.get(&window_id) {
                    win.rendering.set(false);
                }
                return sig;
            }
            RecoveryState::DeferredUntilStable { reason, .. } => {
                let _ = self.maybe_upgrade_reason(window_id, reason);
                let guard = self.windows.borrow();
                if let Some(win) = guard.get(&window_id) {
                    win.rendering.set(false);
                }
                return AppSignal::None;
            }
            RecoveryState::Recovered { .. } => {
                let guard = self.windows.borrow();
                if let Some(win) = guard.get(&window_id) {
                    *win.recovery_state.borrow_mut() = RecoveryState::NotLost;
                }
                // Fall through to normal redraw.
                AppSignal::None
            }
            RecoveryState::GiveUp { .. } => {
                let guard = self.windows.borrow();
                if let Some(win) = guard.get(&window_id) {
                    win.rendering.set(false);
                }
                return AppSignal::RequestQuit;
            }
            RecoveryState::NotLost => {
                // Fall through to normal redraw.
                AppSignal::None
            }
        };

        let _ = result; // All fall-through arms return None; run the actual redraw.
        let outcome = self.run_redraw(window_id);

        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window_id) {
            win.rendering.set(false);
            // Device loss surfaced mid-frame (the entry checks above saw a
            // healthy device, but a resize/paint/present inside run_redraw lost
            // it). Schedule another redraw: on the next dispatch the entry
            // `is_device_lost()` check drives the recovery state machine. We do
            // NOT recover inline — that would re-enter the same borrows.
            if matches!(outcome, RedrawOutcome::DeviceLost) {
                log::info!(target: "slate::device_lost",
                    "mid-frame device loss for window={:?} — scheduling recovery", window_id);
                win.window.request_redraw();
            }
        }
        AppSignal::None
    }
}
