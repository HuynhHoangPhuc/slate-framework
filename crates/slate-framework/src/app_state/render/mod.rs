//! Render pipeline, device-lost recovery, and platform render delegate.
//!
//! Submodules:
//! - `dispatch`: re-entrancy-guarded redraw entry that drives the device-lost
//!   recovery state machine before delegating to `run_redraw`.
//! - `redraw`: inner layout → prepaint → paint → render pipeline.
//! - `surface`: initial bring-up, sync resize, and device-lost / device-restored
//!   platform event arms.
//! - `recovery`: classification + retry helpers driven by `dispatch_redraw`.
//!
//! This `mod.rs` also hosts the `WindowRenderDelegate` impl (sync
//! resize/redraw callbacks from the platform) and the unmount-driven
//! capture-release helper used by the prepaint pass.

use std::time::Instant;

use slate_platform::{PhysicalSize, Window, WindowId, WindowRenderDelegate};

use crate::hit_test::HitTestList;
use crate::view::View;

use super::guards::SyncResizeGuard;
use super::state::AppState;
use super::types::{AppSignal, RecoveryState};

mod dispatch;
mod recovery;
mod redraw;
mod surface;

impl<V: View> AppState<V> {
    /// Clear an active capture if its target produced no hit region this frame
    /// (i.e. the element was unmounted). Called from the prepaint pass after
    /// the hit list is rebuilt.
    pub(super) fn release_capture_if_unmounted(&self, hit: &HitTestList) {
        let captured = *self.capture_target.borrow();
        if let Some(cap_id) = captured
            && !hit.contains(cap_id)
        {
            *self.capture_target.borrow_mut() = None;
            *self.explicit_capture.borrow_mut() = false;
        }
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
                "on_display_change: device probe found loss → requesting redraw");
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
                "exit size/move — resuming recovery via cooldown gate (reason={:?})", reason);
            *self.recovery_state.borrow_mut() = RecoveryState::CooldownGate {
                since: Instant::now(),
                reason,
            };
            self.last_adapter_check_at.set(None);
            self.window.request_redraw();
        }
    }
}
