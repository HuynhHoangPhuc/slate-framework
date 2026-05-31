//! Focus-driven "scroll into view".
//!
//! When keyboard navigation moves focus to a focusable descendant that sits
//! outside the visible viewport, the ScrollView nudges its offset so the
//! newly-focused element becomes visible. This is the substance of
//! [`AccessibilityAction::ScrollIntoView`](crate::types::AccessibilityAction)
//! for the focus path: every redraw re-runs prepaint with the up-to-date
//! focused id, so ScrollView detects the focus *transition* here and sets its
//! caller-owned offset `Signal` (Strategy-A → the reveal lands on the next
//! frame). Routing a raw `accesskit::ActionRequest` from the OS into this same
//! mechanism is a platform-adapter concern handled separately; once a
//! screen-reader focus request maps to `set_focus`, this reveal covers it too.
//!
//! Reveal fires only on a focus *change*, tracked in a persistent state slot —
//! never every frame. Otherwise a user who manually scrolled a focused element
//! out of view would have it yanked back under them.

use std::collections::HashMap;

use crate::context::PrepaintCtx;
use crate::reactive::Signal;
use crate::types::{Bounds, ElementId};

/// Per-ScrollView reveal bookkeeping, held in a persistent
/// [`StateRegistry`](crate::reactive_state::StateRegistry) slot so it survives
/// the whole-view rebuild each `offset.set` triggers.
#[derive(Clone, Copy, Default)]
pub(super) struct RevealState {
    /// The descendant most recently scrolled into view. While focus stays on it
    /// nothing further happens (manual scrolling isn't fought); reset to `None`
    /// when focus leaves this ScrollView so a later refocus reveals again.
    pub last_revealed: Option<ElementId>,
}

/// Compute the offset that brings `target` into `viewport`, or `None` if it
/// already fits (no scroll needed).
///
/// `current_offset` is this frame's clamped scroll offset; `target` is the
/// focused element's on-screen bounds (already translated by the current
/// offset). A target below the viewport scrolls down by the overflow; one above
/// scrolls up. A target taller than the viewport aligns to its top (the top
/// check runs last and wins) so reading starts at the beginning. The result is
/// clamped to `[0, max_offset]`.
pub(super) fn compute_reveal_offset(
    viewport: Bounds,
    target: Bounds,
    current_offset: f32,
    max_offset: f32,
) -> Option<f32> {
    let v_top = viewport.origin.y;
    let v_bottom = v_top + viewport.size.height;
    let t_top = target.origin.y;
    let t_bottom = t_top + target.size.height;

    let mut next = current_offset;
    if t_bottom > v_bottom {
        next = current_offset + (t_bottom - v_bottom);
    }
    if t_top < v_top {
        next = current_offset - (v_top - t_top);
    }

    let clamped = next.clamp(0.0, max_offset.max(0.0));
    if (clamped - current_offset).abs() > f32::EPSILON {
        Some(clamped)
    } else {
        None
    }
}

/// Walk the parent chain upward from `node`; `true` if `ancestor` is on it.
fn is_descendant(
    parent_map: &HashMap<ElementId, ElementId>,
    ancestor: ElementId,
    node: ElementId,
) -> bool {
    let mut cur = node;
    while let Some(&parent) = parent_map.get(&cur) {
        if parent == ancestor {
            return true;
        }
        cur = parent;
    }
    false
}

/// On a focus *transition* to an off-screen focusable descendant, scroll it into
/// view by setting `offset`.
///
/// Call during prepaint, after the children have been prepainted (so their
/// parent links and focus bounds exist) and inside the ScrollView's pushed
/// frame. `viewport` is the ScrollView's bounds; `current_offset` / `max_offset`
/// are this frame's resolved scroll geometry.
pub(super) fn reveal_focused_descendant(
    cx: &mut PrepaintCtx,
    scroll_id: ElementId,
    offset: &Signal<f32>,
    viewport: Bounds,
    current_offset: f32,
    max_offset: f32,
) {
    // Touch the slot every frame so it isn't GC'd while the ScrollView lives.
    let state = cx
        .state_registry
        .use_state::<RevealState>(scroll_id, RevealState::default);

    let focused = cx.focused_element();
    let target = match focused {
        Some(id) if is_descendant(cx.parent_map, scroll_id, id) => Some(id),
        _ => None,
    };

    let Some(focused) = target else {
        // Focus is elsewhere — reset so refocusing a child reveals it again.
        if state.get().last_revealed.is_some() {
            state.set(RevealState::default());
        }
        return;
    };

    if state.get().last_revealed == Some(focused) {
        // Already reacted to this focus; leave any manual scroll alone.
        return;
    }
    // Mark this focus handled regardless of whether a scroll is actually needed
    // (an already-visible focused child still counts as "reacted to"). This is
    // what bounds reveal to a single frame: the `offset.set` below requests the
    // next frame, and on that frame `last_revealed == focused` so we early-return
    // above — exactly one reveal per focus change, never a loop.
    state.set(RevealState {
        last_revealed: Some(focused),
    });

    if let Some(bounds) = cx.focus_bounds.get(&focused).map(|fb| fb.bounds)
        && let Some(next) = compute_reveal_offset(viewport, bounds, current_offset, max_offset)
    {
        offset.set(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Point, Size};

    fn viewport() -> Bounds {
        Bounds::new(Point::new(0.0, 0.0), Size::new(100.0, 100.0))
    }

    fn rect(y: f32, h: f32) -> Bounds {
        Bounds::new(Point::new(0.0, y), Size::new(100.0, h))
    }

    #[test]
    fn fully_visible_target_needs_no_scroll() {
        assert_eq!(compute_reveal_offset(viewport(), rect(10.0, 40.0), 0.0, 200.0), None);
    }

    #[test]
    fn target_below_viewport_scrolls_down_by_overflow() {
        // Target bottom at 150, viewport bottom 100 → scroll down 50.
        let next = compute_reveal_offset(viewport(), rect(110.0, 40.0), 0.0, 200.0);
        assert_eq!(next, Some(50.0));
    }

    #[test]
    fn target_above_viewport_scrolls_up() {
        // Target top at -30 (scrolled past), viewport top 0 → scroll up 30
        // from the current offset of 80 → 50.
        let next = compute_reveal_offset(viewport(), rect(-30.0, 40.0), 80.0, 200.0);
        assert_eq!(next, Some(50.0));
    }

    #[test]
    fn target_taller_than_viewport_aligns_to_top() {
        // 160px target straddling the viewport: bottom check would scroll down,
        // but the top check runs last and wins → align the target's top.
        let next = compute_reveal_offset(viewport(), rect(-20.0, 160.0), 40.0, 200.0);
        assert_eq!(next, Some(20.0));
    }

    #[test]
    fn reveal_offset_clamps_to_max() {
        // Overflow would push past max_offset 30 → clamp.
        let next = compute_reveal_offset(viewport(), rect(200.0, 40.0), 0.0, 30.0);
        assert_eq!(next, Some(30.0));
    }

    #[test]
    fn is_descendant_direct_child() {
        let mut pm = HashMap::new();
        let scroll = ElementId::from_raw(1);
        let child = ElementId::from_raw(2);
        pm.insert(child, scroll);
        assert!(is_descendant(&pm, scroll, child));
    }

    #[test]
    fn is_descendant_grandchild() {
        let mut pm = HashMap::new();
        let scroll = ElementId::from_raw(1);
        let child = ElementId::from_raw(2);
        let grandchild = ElementId::from_raw(3);
        pm.insert(child, scroll);
        pm.insert(grandchild, child);
        assert!(is_descendant(&pm, scroll, grandchild));
    }

    #[test]
    fn is_descendant_rejects_unrelated_and_self() {
        let mut pm = HashMap::new();
        let scroll = ElementId::from_raw(1);
        let other = ElementId::from_raw(9);
        pm.insert(other, ElementId::from_raw(8));
        assert!(!is_descendant(&pm, scroll, other));
        // An element is not its own descendant.
        assert!(!is_descendant(&pm, scroll, scroll));
    }
}
