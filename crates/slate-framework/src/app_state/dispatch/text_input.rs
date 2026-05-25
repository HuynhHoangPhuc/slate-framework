//! Text input dispatch.
//!
//! Same focused-chain bubble shape as keyboard dispatch — the platform layer
//! composes text from `WM_CHAR`/IME commit/etc. and hands a finalized `String`
//! to this dispatcher.

use std::time::Instant;

use slate_platform::WindowId;
use smallvec::SmallVec;

use crate::event::{
    ElementTextInputHandler, EventCtx, PendingCaptureOp, PendingFocusOp, TextInputEvent,
};
use crate::types::ElementId;

use super::super::state::AppState;
use super::super::types::AppSignal;

impl AppState {
    /// Dispatch `TextInput`: same bubble shape; composes text from the platform layer.
    pub(crate) fn dispatch_text_input(&self, window: WindowId, text: String) -> AppSignal {
        let has_app_handlers = !self.on_text_input.borrow().is_empty();
        let chain = self.build_focused_chain(window);
        if chain.is_empty() && !has_app_handlers {
            return AppSignal::None;
        }

        let event = TextInputEvent {
            text,
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut pending_focus_op: Option<PendingFocusOp> = None;
        let mut pending_capture_op: Option<PendingCaptureOp> = None;

        let focused = {
            let guard = self.windows.borrow();
            guard.get(&window).and_then(|w| w.focus_registry.borrow().focused())
        };

        // Snapshot per-element handlers up-front to avoid holding map borrow
        // while invoking user code.
        let element_handlers: SmallVec<[(ElementId, ElementTextInputHandler); 8]> = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else {
                return AppSignal::None;
            };
            let map = win.key_handler_map.borrow();
            chain
                .iter()
                .filter_map(|id| {
                    map.get(id)
                        .and_then(|h| h.on_text_input.clone())
                        .map(|h| (*id, h))
                })
                .collect()
        };

        for (id, handler) in &element_handlers {
            let ime_rc = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else { break };
                win.ime_registry.clone()
            };
            let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    window,
                    focused,
                )
                .with_ime(*id, &ime_rc);
            handler(&event, &mut ctx);
            if stopped {
                break;
            }
        }

        if !stopped && has_app_handlers {
            let mut handlers = self.on_text_input.borrow_mut();
            for handler in handlers.iter_mut() {
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    window,
                    focused,
                );
                handler(&event, &mut ctx);
                if stopped {
                    break;
                }
            }
            drop(handlers);
        }

        self.apply_pending_focus_op(window, pending_focus_op);
        self.apply_pending_capture_op(window, pending_capture_op);
        AppSignal::RequestRedraw { window }
    }
}
