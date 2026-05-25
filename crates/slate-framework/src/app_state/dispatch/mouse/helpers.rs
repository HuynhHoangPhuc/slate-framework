//! Shared chain-walking helpers used by every mouse dispatcher sibling.
//!
//! `bubble_mouse_handler` / `bubble_pointer_handler` / `bubble_scroll_handler`
//! are currently unreferenced after the dispatcher distribution, but kept here
//! verbatim from the monolith so that future call sites (e.g. synthetic event
//! injection paths) have a uniform bubble primitive without re-deriving the
//! `EventCtx` plumbing. The `#![allow(dead_code)]` at the `app_state` mod
//! root covers them.

use std::collections::HashMap;
use std::time::Instant;

use slate_platform::{Modifiers, MouseButton, WindowId};
use smallvec::SmallVec;

use crate::event::{
    EventCtx, Handlers, MouseEvent, MouseHandler, PendingCaptureOp, PendingFocusOp, PointerEvent,
    PointerEventKind, PointerHandler, ScrollEvent, ScrollHandler,
};
use crate::types::ElementId;

/// Walk ancestors from `start` to root, yielding each ElementId.
pub(super) fn ancestors<'a>(
    start: ElementId,
    parent_map: &'a HashMap<ElementId, ElementId>,
) -> impl Iterator<Item = ElementId> + 'a {
    let mut current = Some(start);
    std::iter::from_fn(move || {
        let id = current?;
        current = parent_map.get(&id).copied();
        Some(id)
    })
}

/// Bubble a mouse event through the ancestor chain, invoking handlers.
pub(super) fn bubble_mouse_handler<F>(
    target: ElementId,
    event: &MouseEvent,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
    window_id: WindowId,
    get_handler: F,
) where
    F: Fn(&Handlers) -> Option<MouseHandler>,
{
    let mut chain: SmallVec<[MouseHandler; 8]> = SmallVec::new();
    for id in ancestors(target, parent_map) {
        if let Some(h) = handler_map
            .get(&id)
            .and_then(|handlers| get_handler(handlers))
        {
            chain.push(h);
        }
    }

    let mut stopped = false;
    let mut pending_focus_op: Option<PendingFocusOp> = None;
    let mut pending_capture_op: Option<PendingCaptureOp> = None;
    for handler in &chain {
        let mut ctx = EventCtx::new(
            &mut stopped,
            &mut pending_focus_op,
            &mut pending_capture_op,
            window_id,
            None,
        );
        handler(event, &mut ctx);
        if stopped {
            break;
        }
    }
    // `pending_focus_op` is not applied here; these free-standing bubble
    // helpers have no AppState reference. Callers that need focus applied
    // (e.g. mouse-down) drain it via `apply_pending_focus_op` after return.
    let _ = pending_focus_op;
}

/// Bubble a pointer event through the ancestor chain, invoking handlers.
pub(super) fn bubble_pointer_handler<F>(
    target: ElementId,
    event: &PointerEvent,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
    window_id: WindowId,
    get_handler: F,
) where
    F: Fn(&Handlers) -> Option<PointerHandler>,
{
    let mut chain: SmallVec<[PointerHandler; 8]> = SmallVec::new();
    for id in ancestors(target, parent_map) {
        if let Some(h) = handler_map
            .get(&id)
            .and_then(|handlers| get_handler(handlers))
        {
            chain.push(h);
        }
    }

    let mut stopped = false;
    let mut pending_focus_op: Option<PendingFocusOp> = None;
    let mut pending_capture_op: Option<PendingCaptureOp> = None;
    for handler in &chain {
        let mut ctx = EventCtx::new(
            &mut stopped,
            &mut pending_focus_op,
            &mut pending_capture_op,
            window_id,
            None,
        );
        handler(event, &mut ctx);
        if stopped {
            break;
        }
    }
    let _ = pending_focus_op;
}

/// Bubble a scroll event through the ancestor chain, invoking handlers.
pub(super) fn bubble_scroll_handler(
    target: ElementId,
    event: &ScrollEvent,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
    window_id: WindowId,
) {
    let mut chain: SmallVec<[ScrollHandler; 8]> = SmallVec::new();
    for id in ancestors(target, parent_map) {
        if let Some(h) = handler_map
            .get(&id)
            .and_then(|handlers| handlers.on_mouse_scrolled.clone())
        {
            chain.push(h);
        }
    }

    let mut stopped = false;
    let mut pending_focus_op: Option<PendingFocusOp> = None;
    let mut pending_capture_op: Option<PendingCaptureOp> = None;
    for handler in &chain {
        let mut ctx = EventCtx::new(
            &mut stopped,
            &mut pending_focus_op,
            &mut pending_capture_op,
            window_id,
            None,
        );
        handler(event, &mut ctx);
        if stopped {
            break;
        }
    }
    let _ = pending_focus_op;
}

/// Fire hover transitions between old and new hover targets.
pub(super) fn fire_hover_transitions(
    old_hover: Option<ElementId>,
    new_hover: Option<ElementId>,
    handler_map: &HashMap<ElementId, Handlers>,
    parent_map: &HashMap<ElementId, ElementId>,
    window_id: WindowId,
) {
    use std::collections::HashSet;

    let old_chain: SmallVec<[ElementId; 16]> = if let Some(id) = old_hover {
        ancestors(id, parent_map).collect()
    } else {
        SmallVec::new()
    };
    let new_chain: SmallVec<[ElementId; 16]> = if let Some(id) = new_hover {
        ancestors(id, parent_map).collect()
    } else {
        SmallVec::new()
    };

    let new_set: HashSet<ElementId> = new_chain.iter().copied().collect();
    let old_set: HashSet<ElementId> = old_chain.iter().copied().collect();

    let mut leave_handlers: SmallVec<[PointerHandler; 8]> = SmallVec::new();
    for &id in &old_chain {
        if !new_set.contains(&id)
            && let Some(h) = handler_map
                .get(&id)
                .and_then(|h| h.on_pointer_leave.clone())
        {
            leave_handlers.push(h);
        }
    }

    let mut enter_ids: SmallVec<[ElementId; 8]> = SmallVec::new();
    for &id in &new_chain {
        if !old_set.contains(&id) {
            enter_ids.push(id);
        }
    }
    enter_ids.reverse();

    let mut enter_handlers: SmallVec<[PointerHandler; 8]> = SmallVec::new();
    for &id in &enter_ids {
        if let Some(h) = handler_map
            .get(&id)
            .and_then(|h| h.on_pointer_enter.clone())
        {
            enter_handlers.push(h);
        }
    }

    for handler in &leave_handlers {
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
        let mut ctx = EventCtx::new(
            &mut stopped,
            &mut pending_focus_op,
            &mut pending_capture_op,
            window_id,
            None,
        );
        handler(&event, &mut ctx);
    }

    for handler in &enter_handlers {
        let event = PointerEvent {
            kind: PointerEventKind::Enter,
            position: (0.0, 0.0),
            button: None,
            modifiers: Modifiers::default(),
            timestamp: Instant::now(),
        };
        let mut stopped = false;
        let mut pending_focus_op: Option<PendingFocusOp> = None;
        let mut pending_capture_op: Option<PendingCaptureOp> = None;
        let mut ctx = EventCtx::new(
            &mut stopped,
            &mut pending_focus_op,
            &mut pending_capture_op,
            window_id,
            None,
        );
        handler(&event, &mut ctx);
    }
}

/// Convert MouseButton to bitmask bit position.
pub(super) fn button_to_bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1 << 0,
        MouseButton::Right => 1 << 1,
        MouseButton::Middle => 1 << 2,
        MouseButton::Other(n) => 1 << (3 + n.min(4)),
    }
}
