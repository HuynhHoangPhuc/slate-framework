//! Mouse motion dispatch: `MouseMoved`, `MouseScrolled`, `MouseExited`,
//! plus the coalesced-move flush + hover-state refresh helpers driven from
//! the render pass.

use std::time::Instant;

use slate_platform::{Modifiers, Window, WindowId};
use smallvec::SmallVec;

use crate::event::{
    EventCtx, MouseEvent, MouseHandler, PendingCaptureOp, PendingFocusOp, PointerEvent,
    PointerEventKind, PointerHandler, ScrollEvent, ScrollHandler,
};
use crate::types::{ElementId, Point};
use crate::view::View;

use super::super::super::state::AppState;
use super::super::super::types::AppSignal;
use super::helpers::{ancestors, fire_hover_transitions};

impl<V: View> AppState<V> {
    /// Dispatch MouseMoved event.
    pub(crate) fn dispatch_mouse_moved(
        &self,
        window: WindowId,
        position: (f32, f32),
        modifiers: Modifiers,
    ) -> AppSignal {
        let pointer_event = PointerEvent {
            kind: PointerEventKind::Move,
            position,
            button: None,
            modifiers,
            timestamp: Instant::now(),
        };

        // Route through capture_target if captured, else hit-test
        let captured = *self.capture_target.borrow();
        let target = if let Some(ct) = captured {
            Some(ct)
        } else {
            self.hit_test_list
                .borrow()
                .hit_test(Point::new(position.0, position.1))
                .map(|r| r.element_id)
        };

        if let Some(t) = target {
            // Collect handlers (clone-before-drop pattern)
            let handlers: SmallVec<[PointerHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(t, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_event.clone()))
                    .collect()
            };

            // Invoke handlers
            let mut stopped = false;
            let mut pending_focus_op: Option<PendingFocusOp> = None;
            let mut pending_capture_op: Option<PendingCaptureOp> = None;
            let focused = self.focus_registry.borrow().focused();
            for handler in &handlers {
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
            self.apply_pending_focus_op(pending_focus_op);
            self.apply_pending_capture_op(pending_capture_op);
        }

        *self.coalesced_move_pos.borrow_mut() = Some(position);
        *self.last_mouse_pos.borrow_mut() = Some(position);
        // A move while a capture target is set means a button is held (drag in
        // progress). Request a redraw so the render pass runs
        // `flush_coalesced_move`, which dispatches the coalesced move to the
        // drag handler. Idle hover (no capture) stays silent to avoid a
        // redraw-storm on every pointer move.
        if captured.is_some() {
            AppSignal::RequestRedraw { window }
        } else {
            AppSignal::None
        }
    }

    /// Dispatch MouseScrolled event.
    pub(crate) fn dispatch_mouse_scrolled(
        &self,
        window: WindowId,
        position: (f32, f32),
        delta_x: f32,
        delta_y: f32,
        precise: bool,
        modifiers: Modifiers,
    ) -> AppSignal {
        let scroll_event = ScrollEvent {
            position,
            delta_x,
            delta_y,
            precise,
            modifiers,
            timestamp: Instant::now(),
        };

        let hit = self
            .hit_test_list
            .borrow()
            .hit_test(Point::new(position.0, position.1));

        if let Some(result) = hit {
            // Collect handlers (clone-before-drop pattern)
            let handlers: SmallVec<[ScrollHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                ancestors(result.element_id, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_mouse_scrolled.clone()))
                    .collect()
            };

            // Invoke handlers
            let mut stopped = false;
            let mut pending_focus_op: Option<PendingFocusOp> = None;
            let mut pending_capture_op: Option<PendingCaptureOp> = None;
            let focused = self.focus_registry.borrow().focused();
            for handler in &handlers {
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    window,
                    focused,
                );
                handler(&scroll_event, &mut ctx);
                if stopped {
                    break;
                }
            }
            self.apply_pending_focus_op(pending_focus_op);
            self.apply_pending_capture_op(pending_capture_op);
        }

        AppSignal::RequestRedraw { window }
    }

    /// Dispatch MouseExited event.
    pub(crate) fn dispatch_mouse_exited(&self, window: WindowId) -> AppSignal {
        let old_hover = *self.hovered_element.borrow();
        if old_hover.is_some() {
            // Collect handlers (clone-before-drop pattern)
            let handlers: SmallVec<[PointerHandler; 8]> = {
                let hm = self.handler_map.borrow();
                let pm = self.parent_map.borrow();
                if let Some(id) = old_hover {
                    ancestors(id, &pm)
                        .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_leave.clone()))
                        .collect()
                } else {
                    SmallVec::new()
                }
            };

            // Invoke handlers
            for handler in &handlers {
                let event = PointerEvent {
                    kind: PointerEventKind::Leave,
                    position: (0.0, 0.0),
                    button: None,
                    modifiers: Modifiers::default(),
                    timestamp: Instant::now(),
                };
                let mut stopped = false;
                let mut pending_focus_op: Option<PendingFocusOp> = None;
                let mut pending_capture_op: Option<PendingCaptureOp> = None;
                let focused = self.focus_registry.borrow().focused();
                let mut ctx = EventCtx::new(
                    &mut stopped,
                    &mut pending_focus_op,
                    &mut pending_capture_op,
                    window,
                    focused,
                );
                handler(&event, &mut ctx);
                self.apply_pending_focus_op(pending_focus_op);
                self.apply_pending_capture_op(pending_capture_op);
            }

            *self.hovered_element.borrow_mut() = None;
        }
        *self.last_mouse_pos.borrow_mut() = None;
        *self.coalesced_move_pos.borrow_mut() = None;
        AppSignal::None
    }

    /// Flush a coalesced move to drag handlers. Called from the render pass.
    pub(crate) fn flush_coalesced_move(&self) {
        if let Some(pos) = self.coalesced_move_pos.borrow_mut().take() {
            let last_dispatched = *self.last_dispatched_move_pos.borrow();
            if last_dispatched != Some(pos) {
                let captured = *self.capture_target.borrow();
                let target = if let Some(ct) = captured {
                    Some(ct)
                } else {
                    self.hit_test_list
                        .borrow()
                        .hit_test(Point::new(pos.0, pos.1))
                        .map(|r| r.element_id)
                };

                if let Some(t) = target {
                    let mouse_event = MouseEvent {
                        position: pos,
                        button: None,
                        modifiers: Modifiers::default(),
                        timestamp: Instant::now(),
                    };
                    // Collect from both surfaces (MouseHandlers first per ancestor,
                    // then Handlers) under shared stopped flag — mirrors the
                    // down/up dispatchers so drag tracking on TextField sees the
                    // move before any broad on_mouse_move below. Focused-surface
                    // entries carry their owning id for `ImeState` resolution.
                    let chain: SmallVec<[(Option<ElementId>, MouseHandler); 8]> = {
                        let hm = self.handler_map.borrow();
                        let mhm = self.mouse_handler_map.borrow();
                        let pm = self.parent_map.borrow();
                        let mut acc: SmallVec<[(Option<ElementId>, MouseHandler); 8]> =
                            SmallVec::new();
                        for id in ancestors(t, &pm) {
                            if let Some(h) = mhm.get(&id).and_then(|h| h.on_mouse_move.clone()) {
                                acc.push((Some(id), h));
                            }
                            if let Some(h) = hm.get(&id).and_then(|h| h.on_mouse_move.clone()) {
                                acc.push((None, h));
                            }
                        }
                        acc
                    };
                    let mut stopped = false;
                    let mut pending_focus_op: Option<PendingFocusOp> = None;
                    let mut pending_capture_op: Option<PendingCaptureOp> = None;
                    let focused = self.focus_registry.borrow().focused();
                    for (id_opt, handler) in &chain {
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
                }

                *self.last_dispatched_move_pos.borrow_mut() = Some(pos);
            }
        }
    }

    /// Recompute hover target and fire enter/leave transitions. Called from
    /// the render pass after `flush_coalesced_move`.
    pub(crate) fn update_hover_state(&self) {
        let current_pos = *self.last_mouse_pos.borrow();
        let captured = *self.capture_target.borrow();

        let new_hover = if captured.is_some() {
            captured
        } else if let Some(pos) = current_pos {
            self.hit_test_list
                .borrow()
                .hit_test(Point::new(pos.0, pos.1))
                .map(|r| r.element_id)
        } else {
            None
        };

        let old_hover = *self.hovered_element.borrow();
        if new_hover != old_hover {
            fire_hover_transitions(
                old_hover,
                new_hover,
                &self.handler_map.borrow(),
                &self.parent_map.borrow(),
                self.window.id(),
            );
            *self.hovered_element.borrow_mut() = new_hover;
        }
    }
}
