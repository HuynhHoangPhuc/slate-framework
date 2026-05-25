//! Device-lost recovery helpers driven by `dispatch_redraw`'s state machine.
//!
//! - `classify_loss_reason` and `maybe_upgrade_reason` resolve the origin of
//!   a loss event (wgpu callback vs LUID migration probe).
//! - `execute_recovery_step` and `handle_recovery_failure` advance the retry
//!   loop, recreate the renderer, and fire observers.

use std::rc::Rc;
use std::time::{Duration, Instant};

use slate_platform::Window;
use slate_renderer::{Renderer, RendererObserver};

use crate::view::View;

use super::super::state::AppState;
use super::super::types::{
    AppSignal, DeviceLossReason, RECOVERY_BACKOFF_BASE_MS, RECOVERY_BACKOFF_STEP_MS,
    RECOVERY_MAX_ATTEMPTS, RecoveryState,
};

impl<V: View> AppState<V> {
    /// Classify the origin of a device-loss event by consuming the renderer's
    /// wgpu-callback signal.
    ///
    /// Returns `WgpuCallback` if wgpu's lost-callback fired since the last
    /// consume; otherwise `LuidMigration` (the loss was synthesized by the
    /// per-redraw LUID probe). Must be called on the NotLost→DetectedLost edge.
    pub(super) fn classify_loss_reason(&self) -> DeviceLossReason {
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
    pub(super) fn maybe_upgrade_reason(&self, current: DeviceLossReason) -> DeviceLossReason {
        let callback_fired = self
            .renderer
            .borrow()
            .as_ref()
            .map(|r| r.consume_wgpu_callback_fired())
            .unwrap_or(false);
        if callback_fired && current == DeviceLossReason::LuidMigration {
            self.last_wgpu_callback_loss_at.set(Some(Instant::now()));
            log::info!(target: "slate::device_lost",
                "upgrade-rule: WgpuCallback arrived mid-cycle — upgrading reason from LuidMigration");
            DeviceLossReason::WgpuCallback
        } else {
            current
        }
    }

    /// Execute one step of the recovery retry loop.
    ///
    /// Called when `RecoveryState::Retrying`. Handles backoff sleep, renderer
    /// recreation, observer firing, and state transitions.
    pub(super) fn execute_recovery_step(&self) -> AppSignal {
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
    pub(super) fn handle_recovery_failure(
        &self,
        attempt: u32,
        reason: DeviceLossReason,
    ) -> AppSignal {
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
}
