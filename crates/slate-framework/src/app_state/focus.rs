//! Focus and capture op application.
//!
//! Per-element dispatchers accumulate `PendingFocusOp` / `PendingCaptureOp`
//! through `EventCtx`; the controller drains them through these helpers
//! after each handler chain unwinds (so handlers never observe a partially-
//! applied focus/capture state).

use crate::event::{PendingCaptureOp, PendingFocusOp};
use crate::view::View;

use super::state::AppState;

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
}
