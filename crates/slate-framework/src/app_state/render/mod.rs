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

use super::guards::SyncResizeGuard;
use super::state::AppState;
use super::types::{AppSignal, RecoveryState};
use super::window_state::WindowState;

mod dispatch;
mod recovery;
mod redraw;
mod surface;

pub(crate) use redraw::RedrawOutcome;

impl AppState {
    /// Clear an active capture if its target produced no hit region this frame
    /// (the element was unmounted). Called from the prepaint pass after the
    /// hit list is rebuilt. Operates on `win` directly (borrow already taken
    /// by caller).
    pub(super) fn release_capture_if_unmounted(win: &WindowState, hit: &HitTestList) {
        let captured = *win.capture_target.borrow();
        if let Some(cap_id) = captured
            && !hit.contains(cap_id)
        {
            *win.capture_target.borrow_mut() = None;
            *win.explicit_capture.borrow_mut() = false;
        }
    }
}

// ---------------------------------------------------------------------------
// WindowRenderDelegate impl — sync resize/redraw from platform callbacks
// ---------------------------------------------------------------------------

impl WindowRenderDelegate for AppState {
    fn on_resize_sync(&self, window_id: WindowId, new_size: PhysicalSize) {
        // Resolve window — no-op if destroyed.
        let win_arc = {
            let guard = self.windows.borrow();
            guard.get(&window_id).map(|w| w.window.clone())
        };
        let Some(_win_arc) = win_arc else { return };

        // Hold the sync-resize flag for both the resize and the dispatched
        // redraw so the renderer routes the present through CATransaction::flush().
        // RAII guard clears the flag on Drop even if a panic unwinds.
        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                let _sync_guard = SyncResizeGuard::new(&win.sync_resize);
                // Step 1: resize the swap chain.
                AppState::run_resize_sync_for(win, new_size);
                // Guard must stay alive across dispatch_redraw, so we drop it
                // explicitly after.
                drop(_sync_guard);
            }
        }

        // Step 2: full redraw through the recovery wrapper.
        // Must use dispatch_redraw — sync resize can still hit device-lost.
        if self.dispatch_redraw(window_id) == AppSignal::RequestQuit {
            self.pending_quit.set(true);
        }
    }

    fn on_redraw(&self, window_id: WindowId) {
        if self.dispatch_redraw(window_id) == AppSignal::RequestQuit {
            self.pending_quit.set(true);
        }
    }

    fn on_display_change(&self, window_id: WindowId) {
        log::trace!(target: "slate::device_lost", "on_display_change ENTRY window={:?}", window_id);
        // A monitor-topology change may put the window on a different adapter.
        // Escalate the mismatch now so recovery migrates promptly, rather than
        // waiting for the next throttled per-redraw probe (during which the
        // renderer keeps cross-adapter composing toward a 0x887A0005 removal).
        let migrated = self.probe_adapter_mismatch_and_escalate(window_id);
        // Also catch a genuine device loss the topology change itself caused
        // (not an adapter move) via the health probe.
        let health_lost = {
            let guard = self.windows.borrow();
            guard
                .get(&window_id)
                .and_then(|win| {
                    win.renderer
                        .borrow()
                        .as_ref()
                        .map(|r| r.mark_device_potentially_lost())
                })
                .unwrap_or(false)
        };
        log::trace!(target: "slate::device_lost",
            "on_display_change: migrated={migrated} health_lost={health_lost}");
        if migrated || health_lost {
            log::info!(target: "slate::device_lost",
                "on_display_change: adapter migration or device loss → requesting redraw");
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                win.window.request_redraw();
            }
        }
    }

    fn on_size_move_end(&self, window_id: WindowId) {
        log::trace!(target: "slate::win", "on_size_move_end: modal loop ended window={:?}", window_id);

        let snapshot = {
            let guard = self.windows.borrow();
            guard
                .get(&window_id)
                .map(|win| win.recovery_state.borrow().clone())
        };
        if let Some(RecoveryState::DeferredUntilStable { reason, .. }) = snapshot {
            log::info!(target: "slate::device_lost",
                "exit size/move — resuming recovery via cooldown gate (reason={:?})", reason);
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id) {
                *win.recovery_state.borrow_mut() = RecoveryState::CooldownGate {
                    since: Instant::now(),
                    reason,
                };
                win.last_adapter_check_at.set(None);
                win.window.request_redraw();
            }
        }

        // Apply the final drag size exactly once now that the modal loop ended.
        // During the drag every real `ResizeBuffers` was deferred (the buffers
        // stretched via `DXGI_SCALING_STRETCH`) to avoid the per-frame churn
        // that suspends the device; this is the single crisp reconfigure. The
        // platform issues `on_redraw` immediately after this hook, so the new
        // buffers paint without an extra request here. `Renderer::resize`
        // self-guards on a lost device, so a loss mid-drag falls through to the
        // recovery path above instead of reconfiguring (plan 260615-1516).
        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window_id)
                && let Some(sz) = win.pending_resize.take()
                && let Some(r) = win.renderer.borrow_mut().as_mut()
            {
                r.resize(sz.as_tuple(), win.window.logical_size());
            }
        }

        // Settle point for geometry persistence: a size/move drag has ended, so
        // capture the final geometry now (the per-tick `WindowResized` saves are
        // suppressed mid-drag by the live-resize gate). No-op without a store.
        self.save_window_geometry(window_id);
    }
}
