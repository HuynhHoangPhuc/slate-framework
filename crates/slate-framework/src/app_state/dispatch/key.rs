//! Keyboard event dispatch: `KeyDown` / `KeyUp`.
//!
//! Both dispatchers share the focused-chain bubble shape (snapshot per-element
//! handlers up-front, then walk App-level handlers if propagation was not
//! stopped). `dispatch_key_down` additionally handles the Tab default focus
//! shift and the Tab-during-composition synthetic commit.

use std::time::Instant;

use slate_platform::{Key, KeyCode, Modifiers, NamedKey, WindowId};
use smallvec::SmallVec;

use crate::event::{
    ElementKeyHandler, EventCtx, KeyEvent, KeyHandler, PendingCaptureOp, PendingFocusOp,
    TextInputHandler,
};
use crate::ime::PendingImeOp;
use crate::types::ElementId;

use super::super::state::AppState;
use super::super::types::AppSignal;

impl AppState {
    /// Install App-level keyboard handlers. Called once by `App::run` after
    /// construction and before the platform loop starts.
    pub(crate) fn install_key_handlers(
        &self,
        on_key_down: Vec<KeyHandler>,
        on_key_up: Vec<KeyHandler>,
        on_text_input: Vec<TextInputHandler>,
    ) {
        *self.on_key_down.borrow_mut() = on_key_down;
        *self.on_key_up.borrow_mut() = on_key_up;
        *self.on_text_input.borrow_mut() = on_text_input;
    }

    /// Dispatch `KeyDown`: focused-chain bubble (leaf → root), then App-level
    /// fall-through if propagation was not stopped. Drains pending focus op.
    pub(crate) fn dispatch_key_down(
        &self,
        window: WindowId,
        code: KeyCode,
        key: Key,
        modifiers: Modifiers,
        is_repeat: bool,
    ) -> AppSignal {
        let has_app_handlers = !self.on_key_down.borrow().is_empty();
        let chain = self.build_focused_chain(window);
        if chain.is_empty() && !has_app_handlers {
            return AppSignal::None;
        }

        let event = KeyEvent {
            code,
            key,
            modifiers,
            is_repeat,
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut pending_focus_op: Option<PendingFocusOp> = None;
        let mut pending_capture_op: Option<PendingCaptureOp> = None;

        // Snapshot focused element before dispatching.
        let focused = {
            let guard = self.windows.borrow();
            guard
                .get(&window)
                .and_then(|w| w.focus_registry.borrow().focused())
        };

        // Snapshot per-element handlers up-front so we never hold the
        // `key_handler_map` borrow while invoking user code.
        let element_handlers: SmallVec<[(ElementId, ElementKeyHandler); 8]> = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else {
                return AppSignal::None;
            };
            let map = win.key_handler_map.borrow();
            chain
                .iter()
                .filter_map(|id| {
                    map.get(id)
                        .and_then(|h| h.on_key_down.clone())
                        .map(|h| (*id, h))
                })
                .collect()
        };

        // Clone Rc<RefCell<ImeRegistry>> per iteration so we can drop the
        // outer `windows` borrow before invoking user code (borrow discipline).
        for (id, handler) in &element_handlers {
            let ime_rc = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else { break };
                win.ime_registry.clone() // Rc clone; guard drops after this block
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
            let mut handlers = self.on_key_down.borrow_mut();
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

        // Tab during active composition: synthetically commit the preedit on
        // the still-focused element before the focus shift. Enqueue + drain
        // pattern — mutating IME state inline would conflict with the borrow
        // that focus_registry will take a few lines down.
        if !stopped && matches!(event.key, Key::Named(NamedKey::Tab)) {
            let focused_now = {
                let guard = self.windows.borrow();
                guard
                    .get(&window)
                    .and_then(|w| w.focus_registry.borrow().focused())
            };
            if let Some(focused_id) = focused_now {
                let preedit_text = {
                    let guard = self.windows.borrow();
                    guard.get(&window).and_then(|win| {
                        win.ime_registry
                            .borrow()
                            .get(focused_id)
                            .and_then(|s| s.borrow().preedit.as_ref().map(|p| p.text.clone()))
                    })
                };
                if let Some(text) = preedit_text {
                    {
                        let guard = self.windows.borrow();
                        if let Some(win) = guard.get(&window) {
                            win.pending_ime_ops
                                .borrow_mut()
                                .push(PendingImeOp::Commit { window, text });
                        }
                    }
                    self.drain_pending_ime_ops(window);
                }
            }
        }

        // Tab / Shift+Tab default focus shift. Suppressed when any handler
        // called `cx.stop_propagation()`.
        if !stopped && matches!(event.key, Key::Named(NamedKey::Tab)) {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window) {
                let mut registry = win.focus_registry.borrow_mut();
                let new_id = if event.modifiers.shift {
                    registry.shift_backward()
                } else {
                    registry.shift_forward()
                };
                if let Some(id) = new_id {
                    registry.set_focus(id);
                }
            }
        }

        AppSignal::RequestRedraw { window }
    }

    /// Dispatch `KeyUp`: same bubble shape as `dispatch_key_down`, no Tab default.
    pub(crate) fn dispatch_key_up(
        &self,
        window: WindowId,
        code: KeyCode,
        key: Key,
        modifiers: Modifiers,
    ) -> AppSignal {
        let has_app_handlers = !self.on_key_up.borrow().is_empty();
        let chain = self.build_focused_chain(window);
        if chain.is_empty() && !has_app_handlers {
            return AppSignal::None;
        }

        let event = KeyEvent {
            code,
            key,
            modifiers,
            is_repeat: false,
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut pending_focus_op: Option<PendingFocusOp> = None;
        let mut pending_capture_op: Option<PendingCaptureOp> = None;

        let focused = {
            let guard = self.windows.borrow();
            guard
                .get(&window)
                .and_then(|w| w.focus_registry.borrow().focused())
        };

        let element_handlers: SmallVec<[(ElementId, ElementKeyHandler); 8]> = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else {
                return AppSignal::None;
            };
            let map = win.key_handler_map.borrow();
            chain
                .iter()
                .filter_map(|id| {
                    map.get(id)
                        .and_then(|h| h.on_key_up.clone())
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
            let mut handlers = self.on_key_up.borrow_mut();
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
