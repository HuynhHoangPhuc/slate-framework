//! Mouse button dispatch: `MouseDown`, `MouseUp`, `CaptureLost`.

use std::time::Instant;

use slate_platform::{Modifiers, MouseButton, WindowId};
use smallvec::SmallVec;

use crate::event::{
    EventCtx, MouseEvent, MouseHandler, PendingCaptureOp, PendingFocusOp, PointerEvent,
    PointerEventKind, PointerHandler,
};
use crate::types::{ElementId, Point};

use super::super::super::state::AppState;
use super::super::super::types::AppSignal;
use super::helpers::{ancestors, button_to_bit};

impl AppState {
    /// Dispatch MouseDown event.
    pub(crate) fn dispatch_mouse_down(
        &self,
        window: WindowId,
        position: (f32, f32),
        button: MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
        let mouse_event = MouseEvent {
            position,
            button: Some(button),
            modifiers,
            timestamp: Instant::now(),
        };
        let pointer_event = PointerEvent {
            kind: PointerEventKind::Down,
            position,
            button: Some(button),
            modifiers,
            timestamp: Instant::now(),
        };

        // Update button state.
        let bit = button_to_bit(button);
        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window) {
                let old = *win.button_state.borrow();
                *win.button_state.borrow_mut() = old | bit;
            }
        }

        // Determine dispatch target via capture or hit-test.
        let target = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else {
                return AppSignal::RequestRedraw { window };
            };
            let captured = *win.capture_target.borrow();
            if let Some(ct) = captured {
                Some(ct)
            } else {
                let hit = win
                    .hit_test_list
                    .borrow()
                    .hit_test(Point::new(position.0, position.1));
                if let Some(result) = hit {
                    *win.capture_target.borrow_mut() = Some(result.element_id);
                    Some(result.element_id)
                } else {
                    None
                }
            }
        };

        // Click-to-focus: auto-focus the deepest focusable ancestor on the hit
        // chain BEFORE invoking handlers, so handlers observe updated
        // `focused_element()`. Background-click clears focus.
        let focus_target: Option<ElementId> = if let Some(t) = target {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else {
                return AppSignal::RequestRedraw { window };
            };
            let registry = win.focus_registry.borrow();
            let pm = win.parent_map.borrow();
            ancestors(t, &pm).find(|id| registry.is_focusable(*id))
        } else {
            None
        };
        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window) {
                let mut registry = win.focus_registry.borrow_mut();
                match focus_target {
                    Some(id) => {
                        registry.set_focus(id);
                    }
                    None => registry.clear_focus(),
                }
            }
        }

        if let Some(t) = target {
            // Snapshot handlers before invoking to avoid re-borrow during user code.
            let mouse_handlers: SmallVec<[(Option<ElementId>, MouseHandler); 8]> = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else {
                    return AppSignal::RequestRedraw { window };
                };
                let hm = win.handler_map.borrow();
                let mhm = win.mouse_handler_map.borrow();
                let pm = win.parent_map.borrow();
                let mut acc: SmallVec<[(Option<ElementId>, MouseHandler); 8]> = SmallVec::new();
                for id in ancestors(t, &pm) {
                    if let Some(h) = mhm.get(&id).and_then(|h| h.on_mouse_down.clone()) {
                        acc.push((Some(id), h));
                    }
                    if let Some(h) = hm.get(&id).and_then(|h| h.on_mouse_down.clone()) {
                        acc.push((None, h));
                    }
                }
                acc
            };
            let pointer_handlers: SmallVec<[PointerHandler; 8]> = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else {
                    return AppSignal::RequestRedraw { window };
                };
                let hm = win.handler_map.borrow();
                let pm = win.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_event.clone()))
                    .collect()
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

            for (id_opt, handler) in &mouse_handlers {
                let ime_rc_opt = if id_opt.is_some() {
                    let guard = self.windows.borrow();
                    let Some(win) = guard.get(&window) else { break };
                    Some(win.ime_registry.clone())
                } else {
                    None
                };
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    window,
                    focused,
                );
                if let (Some(id), Some(ime_rc)) = (id_opt, &ime_rc_opt) {
                    ctx = ctx.with_ime(*id, ime_rc);
                }
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }
            self.apply_pending_focus_op(window, pending_focus_op);
            self.apply_pending_capture_op(window, pending_capture_op);

            stopped = false;
            let mut pending_focus_op: Option<PendingFocusOp> = None;
            let mut pending_capture_op: Option<PendingCaptureOp> = None;
            let focused = {
                let guard = self.windows.borrow();
                guard
                    .get(&window)
                    .and_then(|w| w.focus_registry.borrow().focused())
            };
            for handler in &pointer_handlers {
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    window,
                    focused,
                );
                handler(&pointer_event, &mut ctx);
                if stopped {
                    break;
                }
            }
            self.apply_pending_focus_op(window, pending_focus_op);
            self.apply_pending_capture_op(window, pending_capture_op);
        }

        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window) {
                *win.last_mouse_pos.borrow_mut() = Some(position);
            }
        }
        AppSignal::RequestRedraw { window }
    }

    /// Dispatch MouseUp event.
    pub(crate) fn dispatch_mouse_up(
        &self,
        window: WindowId,
        position: (f32, f32),
        button: MouseButton,
        modifiers: Modifiers,
    ) -> AppSignal {
        let mouse_event = MouseEvent {
            position,
            button: Some(button),
            modifiers,
            timestamp: Instant::now(),
        };
        let pointer_event = PointerEvent {
            kind: PointerEventKind::Up,
            position,
            button: Some(button),
            modifiers,
            timestamp: Instant::now(),
        };

        let bit = button_to_bit(button);
        let (new_state, captured, up_hit) = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else {
                return AppSignal::RequestRedraw { window };
            };
            let old = *win.button_state.borrow();
            let ns = old & !bit;
            *win.button_state.borrow_mut() = ns;
            let cap = *win.capture_target.borrow();
            let hit = win
                .hit_test_list
                .borrow()
                .hit_test(Point::new(position.0, position.1))
                .map(|r| r.element_id);
            (ns, cap, hit)
        };
        let target = captured.or(up_hit);

        if let Some(t) = target {
            let mouse_up_handlers: SmallVec<[(Option<ElementId>, MouseHandler); 8]> = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else {
                    return AppSignal::RequestRedraw { window };
                };
                let hm = win.handler_map.borrow();
                let mhm = win.mouse_handler_map.borrow();
                let pm = win.parent_map.borrow();
                let mut acc: SmallVec<[(Option<ElementId>, MouseHandler); 8]> = SmallVec::new();
                for id in ancestors(t, &pm) {
                    if let Some(h) = mhm.get(&id).and_then(|h| h.on_mouse_up.clone()) {
                        acc.push((Some(id), h));
                    }
                    if let Some(h) = hm.get(&id).and_then(|h| h.on_mouse_up.clone()) {
                        acc.push((None, h));
                    }
                }
                acc
            };
            let pointer_handlers: SmallVec<[PointerHandler; 8]> = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else {
                    return AppSignal::RequestRedraw { window };
                };
                let hm = win.handler_map.borrow();
                let pm = win.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_event.clone()))
                    .collect()
            };
            let click_handlers: SmallVec<[MouseHandler; 8]> =
                if button == MouseButton::Left && up_hit == captured && captured.is_some() {
                    let guard = self.windows.borrow();
                    let Some(win) = guard.get(&window) else {
                        return AppSignal::RequestRedraw { window };
                    };
                    let hm = win.handler_map.borrow();
                    let pm = win.parent_map.borrow();
                    ancestors(t, &pm)
                        .filter_map(|id| hm.get(&id).and_then(|h| h.on_click.clone()))
                        .collect()
                } else {
                    SmallVec::new()
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

            for (id_opt, handler) in &mouse_up_handlers {
                let ime_rc_opt = if id_opt.is_some() {
                    let guard = self.windows.borrow();
                    let Some(win) = guard.get(&window) else { break };
                    Some(win.ime_registry.clone())
                } else {
                    None
                };
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    window,
                    focused,
                );
                if let (Some(id), Some(ime_rc)) = (id_opt, &ime_rc_opt) {
                    ctx = ctx.with_ime(*id, ime_rc);
                }
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }
            self.apply_pending_focus_op(window, pending_focus_op);
            self.apply_pending_capture_op(window, pending_capture_op);

            stopped = false;
            let mut pending_focus_op: Option<PendingFocusOp> = None;
            let mut pending_capture_op: Option<PendingCaptureOp> = None;
            let focused = {
                let guard = self.windows.borrow();
                guard
                    .get(&window)
                    .and_then(|w| w.focus_registry.borrow().focused())
            };
            for handler in &pointer_handlers {
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    window,
                    focused,
                );
                handler(&pointer_event, &mut ctx);
                if stopped {
                    break;
                }
            }
            self.apply_pending_focus_op(window, pending_focus_op);
            self.apply_pending_capture_op(window, pending_capture_op);

            stopped = false;
            let mut pending_focus_op: Option<PendingFocusOp> = None;
            let mut pending_capture_op: Option<PendingCaptureOp> = None;
            let focused = {
                let guard = self.windows.borrow();
                guard
                    .get(&window)
                    .and_then(|w| w.focus_registry.borrow().focused())
            };
            for handler in &click_handlers {
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    window,
                    focused,
                );
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }
            self.apply_pending_focus_op(window, pending_focus_op);
            self.apply_pending_capture_op(window, pending_capture_op);
        }

        // Release auto-capture when all buttons are up. Explicit capture is
        // sticky and survives mouse-up (set via EventCtx::set_capture).
        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window) {
                if new_state == 0 && !*win.explicit_capture.borrow() {
                    *win.capture_target.borrow_mut() = None;
                }
                *win.last_mouse_pos.borrow_mut() = Some(position);
            }
        }
        AppSignal::RequestRedraw { window }
    }

    /// Dispatch CaptureLost event.
    pub(crate) fn dispatch_capture_lost(&self, window: WindowId) -> AppSignal {
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            *win.capture_target.borrow_mut() = None;
            *win.explicit_capture.borrow_mut() = false;
            *win.button_state.borrow_mut() = 0;
            win.ime_registry.borrow().clear_drag_flags();
        }
        AppSignal::None
    }
}
