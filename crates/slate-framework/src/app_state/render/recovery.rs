//! Device-lost recovery helpers driven by `dispatch_redraw`'s state machine.
//!
//! - `classify_loss_reason` and `maybe_upgrade_reason` resolve the origin of
//!   a loss event (wgpu callback vs LUID migration probe).
//! - `execute_recovery_step` and `handle_recovery_failure` advance the retry
//!   loop, recreate the renderer, and fire observers.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slate_platform::{Window, WindowId};
use slate_renderer::{Renderer, RendererObserver};

use super::super::state::AppState;
use super::super::types::{
    AppSignal, DeviceLossReason, RECOVERY_BACKOFF_BASE_MS, RECOVERY_BACKOFF_STEP_MS,
    RECOVERY_MAX_ATTEMPTS, RecoveryState,
};

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

    /// Execute one step of the recovery retry loop for the given window.
    ///
    /// Called when `RecoveryState::Retrying`. Handles backoff, renderer
    /// recreation, observer firing, and state transitions.
    pub(super) fn execute_recovery_step(&self, window_id: WindowId) -> AppSignal {
        let (attempt, reason) = {
            let guard = self.windows.borrow();
            match guard
                .get(&window_id)
                .map(|w| w.recovery_state.borrow().clone())
            {
                Some(RecoveryState::Retrying {
                    attempt, reason, ..
                }) => (attempt, reason),
                _ => return AppSignal::None,
            }
        };

        // Backoff sleep (except first attempt).
        if attempt > 0 {
            let backoff = RECOVERY_BACKOFF_BASE_MS + (attempt as u64) * RECOVERY_BACKOFF_STEP_MS;
            log::debug!(target: "slate::device_lost", "recovery backoff sleep: {}ms", backoff);
            std::thread::sleep(Duration::from_millis(backoff));
        }

        log::info!(target: "slate::device_lost",
            "attempting GPU device recovery (attempt {}/{})",
            attempt + 1, RECOVERY_MAX_ATTEMPTS);

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
                    return self.handle_recovery_failure(window_id, attempt, reason);
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
                self.handle_recovery_failure(window_id, attempt, reason)
            }
        }
    }

    /// Handle a failed recovery attempt for the given window.
    pub(super) fn handle_recovery_failure(
        &self,
        window_id: WindowId,
        attempt: u32,
        reason: DeviceLossReason,
    ) -> AppSignal {
        let next = attempt + 1;
        if next >= RECOVERY_MAX_ATTEMPTS {
            log::error!(target: "slate::device_lost",
                "recovery exhausted after {} attempts (reason={:?})", next, reason);
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                *win.recovery_state.borrow_mut() = RecoveryState::GiveUp { reason };
            }
            AppSignal::RequestQuit
        } else {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                *win.recovery_state.borrow_mut() = RecoveryState::Retrying {
                    attempt: next,
                    last_attempt_at: Instant::now(),
                    reason,
                };
                win.window.request_redraw();
            }
            AppSignal::None
        }
    }
}
