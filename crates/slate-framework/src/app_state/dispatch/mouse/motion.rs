//! Mouse motion dispatch: `MouseMoved`, `MouseScrolled`, `MouseExited`,
//! plus the coalesced-move flush + hover-state refresh helpers driven from
//! the render pass.

use std::time::Instant;

use slate_platform::{Modifiers, WindowId};
use smallvec::SmallVec;

use crate::event::{
    EventCtx, MouseEvent, MouseHandler, PendingCaptureOp, PendingFocusOp, PointerEvent,
    PointerEventKind, PointerHandler, ScrollEvent, ScrollHandler,
};
use crate::types::{ElementId, Point};

use super::super::super::state::AppState;
use super::super::super::types::AppSignal;
use super::helpers::{ancestors, fire_hover_transitions};

impl AppState {
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

        let (captured, target) = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else { return AppSignal::None };
            let cap = *win.capture_target.borrow();
            let t = if let Some(ct) = cap {
                Some(ct)
            } else {
                win.hit_test_list.borrow()
                    .hit_test(Point::new(position.0, position.1))
                    .map(|r| r.element_id)
            };
            (cap, t)
        };

        if let Some(t) = target {
            let handlers: SmallVec<[PointerHandler; 8]> = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else { return AppSignal::None };
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
                guard.get(&window).and_then(|w| w.focus_registry.borrow().focused())
            };
            for handler in &handlers {
                let mut ctx = EventCtx::new(
                    &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                );
                handler(&pointer_event, &mut ctx);
                if stopped { break; }
            }
            self.apply_pending_focus_op(window, pending_focus_op);
            self.apply_pending_capture_op(window, pending_capture_op);
        }

        {
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window) {
                *win.coalesced_move_pos.borrow_mut() = Some(position);
                *win.last_mouse_pos.borrow_mut() = Some(position);
            }
        }

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

        let hit = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else { return AppSignal::RequestRedraw { window } };
            win.hit_test_list.borrow().hit_test(Point::new(position.0, position.1))
        };

        if let Some(result) = hit {
            let handlers: SmallVec<[ScrollHandler; 8]> = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else { return AppSignal::RequestRedraw { window } };
                let hm = win.handler_map.borrow();
                let pm = win.parent_map.borrow();
                ancestors(result.element_id, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_mouse_scrolled.clone()))
                    .collect()
            };

            let mut stopped = false;
            let mut pending_focus_op: Option<PendingFocusOp> = None;
            let mut pending_capture_op: Option<PendingCaptureOp> = None;
            let focused = {
                let guard = self.windows.borrow();
                guard.get(&window).and_then(|w| w.focus_registry.borrow().focused())
            };
            for handler in &handlers {
                let mut ctx = EventCtx::new(
                    &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                );
                handler(&scroll_event, &mut ctx);
                if stopped { break; }
            }
            self.apply_pending_focus_op(window, pending_focus_op);
            self.apply_pending_capture_op(window, pending_capture_op);
        }

        AppSignal::RequestRedraw { window }
    }

    /// Dispatch MouseExited event.
    pub(crate) fn dispatch_mouse_exited(&self, window: WindowId) -> AppSignal {
        let (old_hover, handlers) = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else { return AppSignal::None };
            let old_hover = *win.hovered_element.borrow();
            let handlers: SmallVec<[PointerHandler; 8]> = if let Some(id) = old_hover {
                let hm = win.handler_map.borrow();
                let pm = win.parent_map.borrow();
                ancestors(id, &pm)
                    .filter_map(|id| hm.get(&id).and_then(|h| h.on_pointer_leave.clone()))
                    .collect()
            } else {
                SmallVec::new()
            };
            (old_hover, handlers)
        };

        if old_hover.is_some() {
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
                let focused = {
                    let guard = self.windows.borrow();
                    guard.get(&window).and_then(|w| w.focus_registry.borrow().focused())
                };
                let mut ctx = EventCtx::new(
                    &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                );
                handler(&event, &mut ctx);
                self.apply_pending_focus_op(window, pending_focus_op);
                self.apply_pending_capture_op(window, pending_capture_op);
            }

            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window) {
                *win.hovered_element.borrow_mut() = None;
            }
        }

        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            *win.last_mouse_pos.borrow_mut() = None;
            *win.coalesced_move_pos.borrow_mut() = None;
        }
        AppSignal::None
    }

    /// Flush a coalesced move to drag handlers. Called from the render pass.
    pub(crate) fn flush_coalesced_move(&self, window: WindowId) {
        let pos = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else { return };
            win.coalesced_move_pos.borrow_mut().take()
        };
        let Some(pos) = pos else { return };

        let last_dispatched = {
            let guard = self.windows.borrow();
            guard.get(&window).and_then(|w| *w.last_dispatched_move_pos.borrow())
        };
        if last_dispatched == Some(pos) {
            return;
        }

        let (captured, target) = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else { return };
            let cap = *win.capture_target.borrow();
            let t = if let Some(ct) = cap {
                Some(ct)
            } else {
                win.hit_test_list.borrow()
                    .hit_test(Point::new(pos.0, pos.1))
                    .map(|r| r.element_id)
            };
            (cap, t)
        };

        if let Some(t) = target {
            let mouse_event = MouseEvent {
                position: pos,
                button: None,
                modifiers: Modifiers::default(),
                timestamp: Instant::now(),
            };

            let chain: SmallVec<[(Option<ElementId>, MouseHandler); 8]> = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else { return };
                let hm = win.handler_map.borrow();
                let mhm = win.mouse_handler_map.borrow();
                let pm = win.parent_map.borrow();
                let mut acc: SmallVec<[(Option<ElementId>, MouseHandler); 8]> = SmallVec::new();
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
            let focused = {
                let guard = self.windows.borrow();
                guard.get(&window).and_then(|w| w.focus_registry.borrow().focused())
            };

            for (id_opt, handler) in &chain {
                let ime_rc_opt = if id_opt.is_some() {
                    let guard = self.windows.borrow();
                    let Some(win) = guard.get(&window) else { break };
                    Some(win.ime_registry.clone())
                } else {
                    None
                };
                let mut ctx = EventCtx::new(
                    &mut stopped, &mut pending_focus_op, &mut pending_capture_op, window, focused,
                );
                if let (Some(id), Some(ime_rc)) = (id_opt, &ime_rc_opt) {
                    ctx = ctx.with_ime(*id, ime_rc);
                }
                handler(&mouse_event, &mut ctx);
                if stopped { break; }
            }
            self.apply_pending_focus_op(window, pending_focus_op);
            self.apply_pending_capture_op(window, pending_capture_op);

            let _ = captured; // used for target resolution above
        }

        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            *win.last_dispatched_move_pos.borrow_mut() = Some(pos);
        }
    }

    /// Recompute hover target and fire enter/leave transitions. Called from
    /// the render pass after `flush_coalesced_move`.
    pub(crate) fn update_hover_state(&self, window: WindowId) {
        let (current_pos, captured, old_hover) = {
            let guard = self.windows.borrow();
            let Some(win) = guard.get(&window) else { return };
            (
                *win.last_mouse_pos.borrow(),
                *win.capture_target.borrow(),
                *win.hovered_element.borrow(),
            )
        };

        let new_hover = if captured.is_some() {
            captured
        } else if let Some(pos) = current_pos {
            let guard = self.windows.borrow();
            guard.get(&window)
                .and_then(|win| {
                    win.hit_test_list.borrow()
                        .hit_test(Point::new(pos.0, pos.1))
                        .map(|r| r.element_id)
                })
        } else {
            None
        };

        if new_hover != old_hover {
            // Snapshot handler_map + parent_map before calling fire_hover_transitions.
            let (handler_map_snap, parent_map_snap) = {
                let guard = self.windows.borrow();
                let Some(win) = guard.get(&window) else { return };
                (win.handler_map.borrow().clone(), win.parent_map.borrow().clone())
            };
            fire_hover_transitions(
                old_hover,
                new_hover,
                &handler_map_snap,
                &parent_map_snap,
                window,
            );
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window) {
                *win.hovered_element.borrow_mut() = new_hover;
            }
        }
    }
}
