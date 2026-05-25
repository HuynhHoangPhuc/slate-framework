//! Mouse button dispatch: `MouseDown`, `MouseUp`, `CaptureLost`.

use std::time::Instant;

use slate_platform::{Modifiers, MouseButton, Window};
use smallvec::SmallVec;

use crate::event::{
    EventCtx, MouseEvent, MouseHandler, PendingCaptureOp, PendingFocusOp, PointerEvent,
    PointerEventKind, PointerHandler,
};
use crate::types::{ElementId, Point};
use crate::view::View;

use super::super::super::state::AppState;
use super::super::super::types::AppSignal;
use super::{ancestors, button_to_bit};

impl<V: View> AppState<V> {
    /// Dispatch MouseDown event.
    pub(crate) fn dispatch_mouse_down(
        &self,
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

        // Update button state
        let bit = button_to_bit(button);
        let old_state = *self.button_state.borrow();
        *self.button_state.borrow_mut() = old_state | bit;

        // Determine dispatch target
        let captured = *self.capture_target.borrow();
        let target = if let Some(ct) = captured {
            Some(ct)
        } else {
            let hit = self
                .hit_test_list
                .borrow()
                .hit_test(Point::new(position.0, position.1));
            if let Some(result) = hit {
                *self.capture_target.borrow_mut() = Some(result.element_id);
                Some(result.element_id)
            } else {
                None
            }
        };

        // Click-to-focus / click-to-blur. Auto-focuses the deepest focusable
        // ancestor on the hit chain BEFORE invoking handlers, so handlers
        // observe the updated `focused_element()`. A background-click — no
        // focusable ancestor on the hit chain, or a hit-test miss — clears
        // focus (native-widget semantics; reverses the earlier "no auto-blur"
        // design lock). Authors who need to keep focus on a draggable can opt
        // into `focusable(true).focus_ring(false)`.
        let focus_target: Option<ElementId> = if let Some(t) = target {
            let registry = self.focus_registry.borrow();
            let pm = self.parent_map.borrow();
            let result = ancestors(t, &pm).find(|id| registry.is_focusable(*id));
            drop(pm);
            drop(registry);
            result
        } else {
            None
        };
        {
            let mut registry = self.focus_registry.borrow_mut();
            match focus_target {
                Some(id) => {
                    registry.set_focus(id);
                }
                None => registry.clear_focus(),
            }
        }

        if let Some(t) = target {
            // Collect handlers first, then invoke (clone-before-drop pattern).
            // Per ancestor, the focused MouseHandlers surface (TextField v2) fires
            // before the broad Handlers surface so editor state observes the click
            // first under a shared `stopped` flag. The MouseHandlers surface
            // carries its owning element id so the handler can resolve its own
            // `ImeState` via `cx.ime_state(id)`; broad handlers carry `None`.
            let mouse_handlers: SmallVec<[(Option<ElementId>, MouseHandler); 8]> = {
                let hm = self.handler_map.borrow();
                let mhm = self.mouse_handler_map.borrow();
                let pm = self.parent_map.borrow();
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
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_event.clone()))
                    .collect()
            };

            // Invoke handlers (borrows released).
            // `pending_focus_op` accumulates focus requests across the chain
            // (last-write-wins); drained and applied after the loop.
            let mut stopped = false;
            let mut pending_focus_op: Option<PendingFocusOp> = None;
            let mut pending_capture_op: Option<PendingCaptureOp> = None;
            let focused = self.focus_registry.borrow().focused();
            for (id_opt, handler) in &mouse_handlers {
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    self.window.id(),
                    focused,
                );
                if let Some(id) = id_opt {
                    ctx = ctx.with_ime(*id, &self.ime_registry);
                }
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }
            self.apply_pending_focus_op(pending_focus_op);
            self.apply_pending_capture_op(pending_capture_op);

            stopped = false;
            let mut pending_focus_op: Option<PendingFocusOp> = None;
            let mut pending_capture_op: Option<PendingCaptureOp> = None;
            let focused = self.focus_registry.borrow().focused();
            for handler in &pointer_handlers {
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    self.window.id(),
                    focused,
                );
                handler(&pointer_event, &mut ctx);
                if stopped {
                    break;
                }
            }
            self.apply_pending_focus_op(pending_focus_op);
            self.apply_pending_capture_op(pending_capture_op);
        }

        *self.last_mouse_pos.borrow_mut() = Some(position);
        AppSignal::RequestRedraw
    }

    /// Dispatch MouseUp event.
    pub(crate) fn dispatch_mouse_up(
        &self,
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

        // Update button state
        let bit = button_to_bit(button);
        let old_state = *self.button_state.borrow();
        let new_state = old_state & !bit;
        *self.button_state.borrow_mut() = new_state;

        // Determine dispatch target
        let captured = *self.capture_target.borrow();
        let up_hit = self
            .hit_test_list
            .borrow()
            .hit_test(Point::new(position.0, position.1))
            .map(|r| r.element_id);
        let target = captured.or(up_hit);

        if let Some(t) = target {
            // Collect handlers (clone-before-drop pattern). MouseHandlers
            // surface fires first per ancestor, then Handlers, under shared
            // stopped flag. Focused-surface entries carry their owning id so
            // the handler can resolve `ImeState`; broad entries carry `None`.
            let mouse_up_handlers: SmallVec<[(Option<ElementId>, MouseHandler); 8]> = {
                let hm = self.handler_map.borrow();
                let mhm = self.mouse_handler_map.borrow();
                let pm = self.parent_map.borrow();
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
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_event.clone()))
                    .collect()
            };
            let click_handlers: SmallVec<[MouseHandler; 8]> =
                if button == MouseButton::Left && up_hit == captured && captured.is_some() {
                    let hm = self.handler_map.borrow();
                    let pm = self.parent_map.borrow();
                    ancestors(t, &pm)
                        .filter_map(|id| hm.get(&id).and_then(|h| h.on_click.clone()))
                        .collect()
                } else {
                    SmallVec::new()
                };

            // Invoke handlers (borrows released).
            let mut stopped = false;
            let mut pending_focus_op: Option<PendingFocusOp> = None;
            let mut pending_capture_op: Option<PendingCaptureOp> = None;
            let focused = self.focus_registry.borrow().focused();
            for (id_opt, handler) in &mouse_up_handlers {
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    self.window.id(),
                    focused,
                );
                if let Some(id) = id_opt {
                    ctx = ctx.with_ime(*id, &self.ime_registry);
                }
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }
            self.apply_pending_focus_op(pending_focus_op);
            self.apply_pending_capture_op(pending_capture_op);

            stopped = false;
            let mut pending_focus_op: Option<PendingFocusOp> = None;
            let mut pending_capture_op: Option<PendingCaptureOp> = None;
            let focused = self.focus_registry.borrow().focused();
            for handler in &pointer_handlers {
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    self.window.id(),
                    focused,
                );
                handler(&pointer_event, &mut ctx);
                if stopped {
                    break;
                }
            }
            self.apply_pending_focus_op(pending_focus_op);
            self.apply_pending_capture_op(pending_capture_op);

            stopped = false;
            let mut pending_focus_op: Option<PendingFocusOp> = None;
            let mut pending_capture_op: Option<PendingCaptureOp> = None;
            let focused = self.focus_registry.borrow().focused();
            for handler in &click_handlers {
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    self.window.id(),
                    focused,
                );
                handler(&mouse_event, &mut ctx);
                if stopped {
                    break;
                }
            }
            self.apply_pending_focus_op(pending_focus_op);
            self.apply_pending_capture_op(pending_capture_op);
        }

        // Release auto-capture when all buttons are up. Explicit capture
        // (`EventCtx::set_capture`) is sticky and survives mouse-up so a
        // handler can drive a multi-step drag across button releases.
        if new_state == 0 && !*self.explicit_capture.borrow() {
            *self.capture_target.borrow_mut() = None;
        }

        *self.last_mouse_pos.borrow_mut() = Some(position);
        AppSignal::RequestRedraw
    }

    /// Dispatch CaptureLost event.
    ///
    /// Mouse capture can be revoked outside the normal mouse_up path (window
    /// loses focus, modal popup steals it, SetCapture stolen by a sibling).
    /// Any `ImeState.dragging` flag set by `mouse_down` must be cleared here
    /// — otherwise the next `mouse_move` over that element would silently
    /// re-extend the selection without a preceding mouse_down.
    pub(crate) fn dispatch_capture_lost(&self) -> AppSignal {
        *self.capture_target.borrow_mut() = None;
        *self.explicit_capture.borrow_mut() = false;
        *self.button_state.borrow_mut() = 0;
        self.ime_registry.borrow().clear_drag_flags();
        AppSignal::None
    }
}
