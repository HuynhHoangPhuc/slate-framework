//! Focus and capture op application.
//!
//! Per-element dispatchers accumulate `PendingFocusOp` / `PendingCaptureOp`
//! through `EventCtx`; the controller drains them through these helpers after
//! each handler chain unwinds (so handlers never observe a partially-applied
//! focus/capture state). Both helpers now take a `WindowId` and route to the
//! correct per-window `focus_registry` / capture state.

use slate_platform::WindowId;

use crate::event::{PendingCaptureOp, PendingFocusOp};

use super::state::AppState;

impl AppState {
    /// Apply a queued focus op for the given window (drained from an `EventCtx`
    /// after the handler chain unwinds). No-op when `op` is `None` or the
    /// window is unknown.
    pub(crate) fn apply_pending_focus_op(&self, window: WindowId, op: Option<PendingFocusOp>) {
        let Some(op) = op else { return };
        let guard = self.windows.borrow();
        let Some(win) = guard.get(&window) else { return };
        let mut reg = win.focus_registry.borrow_mut();
        match op {
            PendingFocusOp::Focus(id) => {
                reg.set_focus(id);
            }
            PendingFocusOp::Blur => reg.clear_focus(),
        }
    }

    /// Apply a queued capture op for the given window (drained from an
    /// `EventCtx` after the handler chain unwinds). No-op when `op` is `None`
    /// or the window is unknown. A `Set` marks the capture explicit so it
    /// survives mouse-up; `Release` clears both the target and the explicit
    /// flag.
    pub(crate) fn apply_pending_capture_op(
        &self,
        window: WindowId,
        op: Option<PendingCaptureOp>,
    ) {
        let Some(op) = op else { return };
        let guard = self.windows.borrow();
        let Some(win) = guard.get(&window) else { return };
        match op {
            PendingCaptureOp::Set(id) => {
                *win.capture_target.borrow_mut() = Some(id);
                *win.explicit_capture.borrow_mut() = true;
            }
            PendingCaptureOp::Release => {
                *win.capture_target.borrow_mut() = None;
                *win.explicit_capture.borrow_mut() = false;
            }
        }
    }
}
