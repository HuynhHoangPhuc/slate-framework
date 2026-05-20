//! Selection-aware editing ops over [`ImeState`](crate::ime::ImeState).
//!
//! Pure state mutations shared by `TextField` and `TextArea`: query the active
//! selection, delete it, apply undo/redo snapshots, record edits on the undo
//! stack, reset the caret blink, and run a motion key with selection
//! semantics. Callers own the `&mut ImeState` borrow scope (borrow discipline:
//! always drop the `RefMut` before calling `value.set`).

use crate::ime::ImeState;

use super::undo::{EditOp, EditSnapshot};

/// Return the active selection range `(lo, hi)` if non-empty.
pub(crate) fn selection_range(state: &ImeState) -> Option<(usize, usize)> {
    let anchor = state.selection_anchor?;
    let lo = state.caret.min(anchor);
    let hi = state.caret.max(anchor);
    (lo < hi).then_some((lo, hi))
}

/// Apply `(text, caret, anchor)` from a restored undo/redo snapshot to
/// `state`. Always clears preedit — undo is incompatible with a live
/// composition window and the platform will be re-notified on the next focus
/// edge. Resets the caret blink so the restored state is visible immediately.
pub(crate) fn apply_snapshot(state: &mut ImeState, snap: &EditSnapshot) {
    state.text = snap.text.clone();
    state.caret = snap.caret.min(state.text.len());
    state.selection_anchor = snap.anchor.map(|a| a.min(state.text.len()));
    state.preedit = None;
    reset_blink(state);
}

/// Record an edit on the element's undo stack. `state` must be the
/// **post-mutation** state — the snapshot is built from it and handed to the
/// coalesce policy in `UndoStack::record_edit`, which decides whether to push a
/// new entry or overwrite the head. Operates on the live `ImeState` borrow, so
/// undo history persists across the per-frame element rebuild.
pub(crate) fn record_edit(state: &mut ImeState, op: EditOp) {
    let snap = EditSnapshot {
        text: state.text.clone(),
        caret: state.caret,
        anchor: state.selection_anchor,
    };
    state.undo.record_edit(op, snap);
}

/// Reset the caret blink so the caret is visible immediately after a
/// caret-affecting event. Paint re-arms the next-toggle deadline on the
/// following frame.
pub(crate) fn reset_blink(state: &mut ImeState) {
    state.blink.visible = true;
    state.blink.next = None;
}

/// Delete the current selection, if any, and collapse caret + clear anchor.
///
/// Returns `true` when a non-empty selection was deleted (so callers know to
/// `value.set`). Empty selections (`anchor == caret`) clear the anchor without
/// mutating text and return `false`.
pub(crate) fn delete_selection(state: &mut ImeState) -> bool {
    let anchor = match state.selection_anchor {
        Some(a) => a,
        None => return false,
    };
    let lo = state.caret.min(anchor);
    let hi = state.caret.max(anchor);
    if lo < hi {
        debug_assert!(state.text.is_char_boundary(lo));
        debug_assert!(state.text.is_char_boundary(hi));
        state.text.replace_range(lo..hi, "");
        state.caret = lo;
        state.selection_anchor = None;
        true
    } else {
        state.selection_anchor = None;
        false
    }
}

/// Direction for motion keys. Determines collapse edge on plain motion when
/// a selection is active.
#[derive(Clone, Copy)]
pub(crate) enum MotionDir {
    Left,
    Right,
}

/// Apply a motion key with selection semantics.
///
/// - `shift = true`: anchor at pre-move caret if no anchor; then run `mover`.
/// - `shift = false` with non-empty selection: collapse to `dir` edge instead
///   of running `mover` (matches AppKit/Win32). Empty selection or no anchor:
///   clear anchor and run `mover` normally.
pub(crate) fn apply_motion(
    state: &mut ImeState,
    dir: MotionDir,
    shift: bool,
    mover: impl FnOnce(&mut ImeState),
) {
    if shift {
        if state.selection_anchor.is_none() {
            state.selection_anchor = Some(state.caret);
        }
        mover(state);
        return;
    }
    // Plain motion
    if let Some(anchor) = state.selection_anchor {
        let lo = state.caret.min(anchor);
        let hi = state.caret.max(anchor);
        state.selection_anchor = None;
        if lo < hi {
            // Non-empty selection: collapse to edge, do not move further.
            state.caret = match dir {
                MotionDir::Left => lo,
                MotionDir::Right => hi,
            };
            return;
        }
    }
    mover(state);
}

/// Move the caret to a precomputed `target` byte with selection semantics, for
/// motions whose destination is resolved by layout rather than a byte-walk
/// (vertical ↑/↓, visual-line Home/End). Unlike [`apply_motion`] there is no
/// directional collapse: plain motion clears the anchor and jumps to `target`;
/// Shift motion anchors at the pre-move caret (if unanchored) and extends to
/// `target`. The caller has already clamped `target` to a valid boundary.
pub(crate) fn apply_motion_to(state: &mut ImeState, shift: bool, target: usize) {
    if shift {
        if state.selection_anchor.is_none() {
            state.selection_anchor = Some(state.caret);
        }
    } else {
        state.selection_anchor = None;
    }
    state.caret = target;
}

// ---------------------------------------------------------------------------
// Unit tests — selection model
// ---------------------------------------------------------------------------

#[cfg(test)]
mod selection_tests {
    use super::*;
    use crate::elements::text_edit::grapheme::{next_grapheme_boundary, prev_grapheme_boundary};

    fn state_with(text: &str, caret: usize, anchor: Option<usize>) -> ImeState {
        ImeState {
            text: text.to_string(),
            caret,
            selection_anchor: anchor,
            ..Default::default()
        }
    }

    #[test]
    fn delete_selection_removes_range_and_collapses() {
        let mut s = state_with("hello world", 5, Some(0));
        assert!(delete_selection(&mut s));
        assert_eq!(s.text, " world");
        assert_eq!(s.caret, 0);
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn delete_selection_caret_before_anchor() {
        let mut s = state_with("abcdef", 1, Some(4));
        assert!(delete_selection(&mut s));
        assert_eq!(s.text, "aef");
        assert_eq!(s.caret, 1);
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn delete_selection_empty_clears_anchor_returns_false() {
        let mut s = state_with("abc", 1, Some(1));
        assert!(!delete_selection(&mut s));
        assert_eq!(s.text, "abc");
        assert_eq!(s.caret, 1);
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn delete_selection_no_anchor_noop() {
        let mut s = state_with("abc", 1, None);
        assert!(!delete_selection(&mut s));
        assert_eq!(s.text, "abc");
        assert_eq!(s.caret, 1);
    }

    #[test]
    fn shift_motion_anchors_at_pre_move_caret() {
        let mut s = state_with("hello", 2, None);
        apply_motion(&mut s, MotionDir::Right, true, |s| {
            s.caret = next_grapheme_boundary(&s.text, s.caret);
        });
        assert_eq!(s.selection_anchor, Some(2));
        assert_eq!(s.caret, 3);
    }

    #[test]
    fn shift_motion_keeps_existing_anchor() {
        let mut s = state_with("hello", 3, Some(1));
        apply_motion(&mut s, MotionDir::Right, true, |s| {
            s.caret = next_grapheme_boundary(&s.text, s.caret);
        });
        assert_eq!(s.selection_anchor, Some(1), "anchor must not move");
        assert_eq!(s.caret, 4);
    }

    #[test]
    fn plain_left_with_selection_collapses_to_left_edge() {
        // selection: caret=4, anchor=1 → range [1..4]. Plain ← collapses to 1.
        let mut s = state_with("abcdef", 4, Some(1));
        apply_motion(&mut s, MotionDir::Left, false, |s| {
            s.caret = prev_grapheme_boundary(&s.text, s.caret);
        });
        assert_eq!(s.caret, 1, "collapses to lo edge, no further move");
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn plain_right_with_selection_collapses_to_right_edge() {
        let mut s = state_with("abcdef", 1, Some(4));
        apply_motion(&mut s, MotionDir::Right, false, |s| {
            s.caret = next_grapheme_boundary(&s.text, s.caret);
        });
        assert_eq!(s.caret, 4, "collapses to hi edge, no further move");
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn plain_motion_no_selection_moves_normally() {
        let mut s = state_with("abc", 1, None);
        apply_motion(&mut s, MotionDir::Right, false, |s| {
            s.caret = next_grapheme_boundary(&s.text, s.caret);
        });
        assert_eq!(s.caret, 2);
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn plain_motion_empty_selection_clears_anchor_and_moves() {
        let mut s = state_with("abc", 1, Some(1));
        apply_motion(&mut s, MotionDir::Right, false, |s| {
            s.caret = next_grapheme_boundary(&s.text, s.caret);
        });
        // Empty anchor cleared, then mover runs → caret advances by one grapheme.
        assert_eq!(s.caret, 2);
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn shift_home_then_shift_end_swaps_selection_around_anchor() {
        let mut s = state_with("hello", 2, None);
        // Shift+Home
        apply_motion(&mut s, MotionDir::Left, true, |s| s.caret = 0);
        assert_eq!(s.selection_anchor, Some(2));
        assert_eq!(s.caret, 0);
        // Shift+End reuses the same anchor; caret moves past anchor.
        apply_motion(&mut s, MotionDir::Right, true, |s| s.caret = s.text.len());
        assert_eq!(s.selection_anchor, Some(2));
        assert_eq!(s.caret, 5);
    }

    #[test]
    fn selection_range_returns_lo_hi_when_non_empty() {
        let s = state_with("abcdef", 4, Some(1));
        assert_eq!(selection_range(&s), Some((1, 4)));
        let s = state_with("abcdef", 1, Some(4));
        assert_eq!(selection_range(&s), Some((1, 4)));
    }

    #[test]
    fn selection_range_none_for_empty_or_unanchored() {
        assert_eq!(selection_range(&state_with("abc", 1, None)), None);
        assert_eq!(selection_range(&state_with("abc", 1, Some(1))), None);
    }

    #[test]
    fn apply_motion_to_plain_jumps_and_clears_anchor() {
        let mut s = state_with("hello", 1, Some(3));
        apply_motion_to(&mut s, false, 4);
        assert_eq!(s.caret, 4);
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn apply_motion_to_shift_anchors_and_extends() {
        let mut s = state_with("hello", 2, None);
        apply_motion_to(&mut s, true, 5);
        assert_eq!(s.selection_anchor, Some(2), "anchors at pre-move caret");
        assert_eq!(s.caret, 5);
        // A second shift motion keeps the same anchor.
        apply_motion_to(&mut s, true, 0);
        assert_eq!(s.selection_anchor, Some(2));
        assert_eq!(s.caret, 0);
    }

    #[test]
    fn grapheme_boundary_collapse_with_cjk() {
        // "こんにちは" — each codepoint is 3 bytes.
        let mut s = state_with("こんにちは", 9, Some(3));
        apply_motion(&mut s, MotionDir::Left, false, |s| {
            s.caret = prev_grapheme_boundary(&s.text, s.caret);
        });
        assert_eq!(s.caret, 3, "collapse must land on a grapheme boundary");
        assert!(s.text.is_char_boundary(s.caret));
    }
}
