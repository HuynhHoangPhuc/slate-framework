//! TextField event handler builders.
//!
//! Each function returns a cloned-closure `Arc<dyn Fn + Send + Sync + 'static>`
//! ready to be stored in `KeyHandlers` / `ImeHandlers`. Closures capture
//! `Signal<String>` by clone so the handlers can fire `value.set()` without
//! holding any `RefMut` on `ImeState` (borrow discipline: always drop the
//! RefMut before calling `value.set`).

use std::sync::Arc;

use slate_reactive::Signal;
use slate_text::byte_at_pixel_x;

use crate::event::{
    ElementImeCommitHandler, ElementImePreeditHandler, ElementKeyHandler, ElementTextInputHandler,
    EventCtx, ImeCommitEvent, ImePreeditEvent, Key, KeyCode, KeyEvent, MouseEvent, MouseHandler,
    NamedKey, TextInputEvent, is_command_modifier,
};
use crate::ime::{ImeState, Preedit};

use super::grapheme::{insert_text_at, next_grapheme_boundary, prev_grapheme_boundary};
use super::{EditOp, EditSnapshot, UndoStackHandle};

/// Return the active selection range `(lo, hi)` if non-empty.
fn selection_range(state: &ImeState) -> Option<(usize, usize)> {
    let anchor = state.selection_anchor?;
    let lo = state.caret.min(anchor);
    let hi = state.caret.max(anchor);
    (lo < hi).then_some((lo, hi))
}

/// Apply `(text, caret, anchor)` from a restored undo/redo snapshot to
/// `state`. Always clears preedit — undo is incompatible with a live
/// composition window and the platform will be re-notified on the next focus
/// edge. Resets the caret blink so the restored state is visible immediately.
fn apply_snapshot(state: &mut ImeState, snap: &EditSnapshot) {
    state.text = snap.text.clone();
    state.caret = snap.caret.min(state.text.len());
    state.selection_anchor = snap.anchor.map(|a| a.min(state.text.len()));
    state.preedit = None;
    reset_blink(state);
}

/// Record an edit on the undo stack. `state` must be the **post-mutation**
/// state. The coalesce policy in `UndoStack::record_edit` decides whether to
/// push a new entry or overwrite the head. Lock contention is non-existent in
/// practice — handlers always run on the main thread — so a poisoned mutex is
/// the only failure mode and is silently ignored to keep typing alive.
fn record_edit(undo: &UndoStackHandle, op: EditOp, state: &ImeState) {
    if let Ok(mut stack) = undo.lock() {
        stack.record_edit(
            op,
            EditSnapshot {
                text: state.text.clone(),
                caret: state.caret,
                anchor: state.selection_anchor,
            },
        );
    }
}

/// Mark the undo stack as having seen a caret-moving event that did not edit
/// text. Used by Arrow / Home / End / mouse handlers to force the next edit
/// to open a fresh undo step (coalesce trigger 2).
fn mark_motion(undo: &UndoStackHandle) {
    if let Ok(mut stack) = undo.lock() {
        stack.mark_motion();
    }
}

/// Reset the caret blink so the caret is visible immediately after a
/// caret-affecting event. Paint re-arms the next-toggle deadline on the
/// following frame.
fn reset_blink(state: &mut ImeState) {
    state.blink.visible = true;
    state.blink.next = None;
}

/// Delete the current selection, if any, and collapse caret + clear anchor.
///
/// Returns `true` when a non-empty selection was deleted (so callers know to
/// `value.set`). Empty selections (`anchor == caret`) clear the anchor without
/// mutating text and return `false`.
fn delete_selection(state: &mut ImeState) -> bool {
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
enum MotionDir {
    Left,
    Right,
}

/// Build the `on_key_down` handler for TextField.
///
/// Handles: Backspace, ←, →, Home, End (with and without Shift).
/// Ignores keys while IME preedit is active (IME owns those keystrokes).
/// Does NOT consume Tab / Enter — they bubble to App level.
pub(super) fn build_key_down_handler(
    value: Signal<String>,
    undo: UndoStackHandle,
) -> ElementKeyHandler {
    Arc::new(move |ev: &KeyEvent, cx: &mut EventCtx| {
        let id = match cx.element_id() {
            Some(i) => i,
            None => return,
        };
        let state_rc = match cx.ime_state(id) {
            Some(s) => s,
            None => return,
        };

        // Modifier shortcuts run BEFORE the IME guard so Paste can abort an
        // active composition (policy R5) and so Undo/Redo work even when a
        // stale preedit somehow persists.
        if is_command_modifier(&ev.modifiers) && !ev.modifiers.alt {
            let shift = ev.modifiers.shift;
            match ev.code {
                KeyCode::KeyC => {
                    let state = state_rc.borrow();
                    if state.preedit.is_some() {
                        return;
                    }
                    if let Some((lo, hi)) = selection_range(&state) {
                        let payload = state.text[lo..hi].to_string();
                        drop(state);
                        slate_platform::clipboard::set_text(&payload);
                    }
                    cx.stop_propagation();
                    return;
                }
                KeyCode::KeyX => {
                    let payload = {
                        let state = state_rc.borrow();
                        if state.preedit.is_some() {
                            return;
                        }
                        selection_range(&state).map(|(lo, hi)| state.text[lo..hi].to_string())
                    };
                    if let Some(text) = payload {
                        slate_platform::clipboard::set_text(&text);
                        let new_text = {
                            let mut state = state_rc.borrow_mut();
                            delete_selection(&mut state);
                            record_edit(&undo, EditOp::Discrete, &state);
                            reset_blink(&mut state);
                            state.text.clone()
                        };
                        value.set(new_text);
                    }
                    cx.stop_propagation();
                    return;
                }
                KeyCode::KeyV => {
                    let pasted = match slate_platform::clipboard::get_text() {
                        Some(t) => t,
                        None => {
                            cx.stop_propagation();
                            return;
                        }
                    };
                    let cleaned: String = pasted
                        .chars()
                        .filter(|c| *c != '\n' && *c != '\r')
                        .collect();
                    if cleaned.is_empty() {
                        cx.stop_propagation();
                        return;
                    }
                    let new_text = {
                        let mut state = state_rc.borrow_mut();
                        // Policy R5: abort any active composition before paste.
                        state.preedit = None;
                        delete_selection(&mut state);
                        let old_caret = state.caret;
                        let new_caret = insert_text_at(&mut state.text, old_caret, &cleaned);
                        state.caret = new_caret;
                        record_edit(&undo, EditOp::Discrete, &state);
                        reset_blink(&mut state);
                        state.text.clone()
                    };
                    cx.stop_propagation();
                    value.set(new_text);
                    return;
                }
                // Undo: Cmd/Ctrl+Z (without shift). Redo on macOS: Cmd+Shift+Z.
                KeyCode::KeyZ => {
                    #[cfg(target_os = "macos")]
                    let is_redo = shift;
                    #[cfg(not(target_os = "macos"))]
                    let is_redo = false;
                    let restored = if let Ok(mut stack) = undo.lock() {
                        if is_redo { stack.redo() } else { stack.undo() }
                    } else {
                        None
                    };
                    let _ = shift; // used on macOS only
                    if let Some(snap) = restored {
                        {
                            let mut state = state_rc.borrow_mut();
                            apply_snapshot(&mut state, &snap);
                        }
                        value.set(snap.text);
                    }
                    cx.stop_propagation();
                    return;
                }
                // Redo on Windows/Linux: Ctrl+Y (without shift).
                #[cfg(not(target_os = "macos"))]
                KeyCode::KeyY if !shift => {
                    let restored = undo.lock().ok().and_then(|mut s| s.redo());
                    if let Some(snap) = restored {
                        {
                            let mut state = state_rc.borrow_mut();
                            apply_snapshot(&mut state, &snap);
                        }
                        value.set(snap.text);
                    }
                    cx.stop_propagation();
                    return;
                }
                _ => {}
            }
        }

        // While IME composition is active, leave all navigation to the IME
        {
            let state = state_rc.borrow();
            if state.preedit.is_some() {
                return;
            }
        }

        let shift = ev.modifiers.shift;
        let new_text: Option<String> = match &ev.key {
            Key::Named(NamedKey::Backspace) => {
                let mut state = state_rc.borrow_mut();
                debug_assert!(
                    state.text.is_char_boundary(state.caret),
                    "TextField caret not on char boundary"
                );
                if state.selection_anchor.is_some_and(|a| a != state.caret) {
                    delete_selection(&mut state);
                    record_edit(&undo, EditOp::Discrete, &state);
                    reset_blink(&mut state);
                    cx.stop_propagation();
                    Some(state.text.clone())
                } else {
                    state.selection_anchor = None;
                    let old_caret = state.caret;
                    let new_caret = prev_grapheme_boundary(&state.text, old_caret);
                    if new_caret < old_caret {
                        state.text.replace_range(new_caret..old_caret, "");
                        state.caret = new_caret;
                        record_edit(&undo, EditOp::Backspace, &state);
                        reset_blink(&mut state);
                        cx.stop_propagation();
                        Some(state.text.clone())
                    } else {
                        None
                    }
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                {
                    let mut state = state_rc.borrow_mut();
                    apply_motion(&mut state, MotionDir::Left, shift, |s| {
                        s.caret = prev_grapheme_boundary(&s.text, s.caret);
                    });
                    reset_blink(&mut state);
                }
                mark_motion(&undo);
                cx.stop_propagation();
                None
            }
            Key::Named(NamedKey::ArrowRight) => {
                {
                    let mut state = state_rc.borrow_mut();
                    apply_motion(&mut state, MotionDir::Right, shift, |s| {
                        s.caret = next_grapheme_boundary(&s.text, s.caret);
                    });
                    reset_blink(&mut state);
                }
                mark_motion(&undo);
                cx.stop_propagation();
                None
            }
            Key::Named(NamedKey::Home) => {
                {
                    let mut state = state_rc.borrow_mut();
                    apply_motion(&mut state, MotionDir::Left, shift, |s| s.caret = 0);
                    reset_blink(&mut state);
                }
                mark_motion(&undo);
                cx.stop_propagation();
                None
            }
            Key::Named(NamedKey::End) => {
                {
                    let mut state = state_rc.borrow_mut();
                    apply_motion(&mut state, MotionDir::Right, shift, |s| {
                        s.caret = s.text.len()
                    });
                    reset_blink(&mut state);
                }
                mark_motion(&undo);
                cx.stop_propagation();
                None
            }
            // Tab / Enter: bubble — do NOT stop propagation
            _ => None,
        };

        if let Some(t) = new_text {
            value.set(t);
        }
    })
}

/// Apply a motion key with selection semantics.
///
/// - `shift = true`: anchor at pre-move caret if no anchor; then run `mover`.
/// - `shift = false` with non-empty selection: collapse to `dir` edge instead
///   of running `mover` (matches AppKit/Win32). Empty selection or no anchor:
///   clear anchor and run `mover` normally.
fn apply_motion(
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

/// Build the `on_text_input` handler for TextField.
///
/// Inserts composed text at the caret for non-IME ASCII input.
/// Ignored when IME preedit is active.
pub(super) fn build_text_input_handler(
    value: Signal<String>,
    undo: UndoStackHandle,
) -> ElementTextInputHandler {
    Arc::new(move |ev: &TextInputEvent, cx: &mut EventCtx| {
        let id = match cx.element_id() {
            Some(i) => i,
            None => return,
        };
        let state_rc = match cx.ime_state(id) {
            Some(s) => s,
            None => return,
        };

        // If IME owns the keystroke, ignore text input
        {
            let state = state_rc.borrow();
            if state.preedit.is_some() {
                return;
            }
        }

        let new_text = {
            let mut state = state_rc.borrow_mut();
            debug_assert!(
                state.text.is_char_boundary(state.caret),
                "TextField caret not on char boundary before text insert"
            );
            // Typing into a selection is a discrete undo step — the
            // selection-delete itself is irreversible without one. Plain
            // typing into the caret coalesces under EditOp::Insert.
            let had_selection = state.selection_anchor.is_some_and(|a| a != state.caret);
            delete_selection(&mut state);
            let old_caret = state.caret;
            let new_caret = insert_text_at(&mut state.text, old_caret, &ev.text);
            state.caret = new_caret;
            let op = if had_selection {
                EditOp::Discrete
            } else {
                EditOp::Insert
            };
            record_edit(&undo, op, &state);
            reset_blink(&mut state);
            state.text.clone()
        };

        cx.stop_propagation();
        value.set(new_text);
    })
}

/// Build the `on_mouse_down` handler for TextField (Phase 10a.5).
///
/// Reads the cached `ShapedLine` + paint origin written by `paint()` on the
/// previous frame, snaps the click x to a grapheme boundary via
/// `slate_text::byte_at_pixel_x`, then anchors a new selection at that byte
/// (collapsed caret today; drag-extension happens in `mouse_move`).
///
/// IME guard: during an active preedit composition, mouse interaction is a
/// no-op — platforms expect `selectedRange == markedRange` while composing,
/// and richer interaction is deferred to a later phase.
pub(super) fn build_mouse_down_handler(undo: UndoStackHandle) -> MouseHandler {
    Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
        let id = match cx.element_id() {
            Some(i) => i,
            None => return,
        };
        let state_rc = match cx.ime_state(id) {
            Some(s) => s,
            None => return,
        };

        {
            let mut state = state_rc.borrow_mut();
            if state.preedit.is_some() {
                return;
            }
            let shaped = match state.last_shaped.clone() {
                Some(s) => s,
                None => return,
            };
            let local_x = ev.position.0 - state.paint_origin_x;
            let byte = byte_at_pixel_x(&shaped, &state.text, local_x);
            debug_assert!(
                state.text.is_char_boundary(byte),
                "byte_at_pixel_x must return a char boundary"
            );
            state.caret = byte;
            state.selection_anchor = Some(byte);
            state.dragging = true;
            reset_blink(&mut state);
        }
        mark_motion(&undo);
        cx.stop_propagation();
    })
}

/// Build the `on_mouse_move` handler for TextField (Phase 10a.5).
///
/// Only extends the active selection while `state.dragging` is true (set by
/// `mouse_down`, cleared by `mouse_up`). The anchor stays put; only the caret
/// moves. Returns early outside a drag or during preedit composition.
pub(super) fn build_mouse_move_handler() -> MouseHandler {
    Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
        let id = match cx.element_id() {
            Some(i) => i,
            None => return,
        };
        let state_rc = match cx.ime_state(id) {
            Some(s) => s,
            None => return,
        };

        let mut state = state_rc.borrow_mut();
        if !state.dragging || state.preedit.is_some() {
            return;
        }
        let shaped = match state.last_shaped.clone() {
            Some(s) => s,
            None => return,
        };
        let local_x = ev.position.0 - state.paint_origin_x;
        let byte = byte_at_pixel_x(&shaped, &state.text, local_x);
        debug_assert!(
            state.text.is_char_boundary(byte),
            "byte_at_pixel_x must return a char boundary"
        );
        state.caret = byte;
        reset_blink(&mut state);
        drop(state);
        cx.stop_propagation();
    })
}

/// Build the `on_mouse_up` handler for TextField (Phase 10a.5).
///
/// Ends the drag. If the anchor and caret coincide, the selection collapses
/// (`selection_anchor` cleared) so subsequent typing replaces the caret rather
/// than the (zero-length) selection. Always runs, even during preedit, so a
/// stuck `dragging` flag from `mouse_down` before composition started can be
/// cleared.
pub(super) fn build_mouse_up_handler() -> MouseHandler {
    Arc::new(move |_ev: &MouseEvent, cx: &mut EventCtx| {
        let id = match cx.element_id() {
            Some(i) => i,
            None => return,
        };
        let state_rc = match cx.ime_state(id) {
            Some(s) => s,
            None => return,
        };

        {
            let mut state = state_rc.borrow_mut();
            state.dragging = false;
            if state.selection_anchor == Some(state.caret) {
                state.selection_anchor = None;
            }
        }
        cx.stop_propagation();
    })
}

/// Build the `on_ime_preedit` handler for TextField.
///
/// Writes the incoming composition into `ImeState.preedit` without mutating
/// `ImeState.text` or firing `value.set` — the preedit is an overlay only.
pub(super) fn build_ime_preedit_handler() -> ElementImePreeditHandler {
    Arc::new(move |ev: &ImePreeditEvent, cx: &mut EventCtx| {
        let id = match cx.element_id() {
            Some(i) => i,
            None => return,
        };
        let state_rc = match cx.ime_state(id) {
            Some(s) => s,
            None => return,
        };

        {
            let mut state = state_rc.borrow_mut();
            if ev.text.is_empty() {
                state.preedit = None;
            } else {
                state.preedit = Some(Preedit {
                    text: ev.text.clone(),
                    cursor_byte_offset: ev.cursor_byte_offset,
                    selection: ev.selection.clone(),
                });
            }
        }

        cx.stop_propagation();
    })
}

/// Build the `on_ime_commit` handler for TextField.
///
/// Empty `text` → clear preedit only (macOS `unmarkText`).
/// Non-empty `text` → insert at caret, advance caret, clear preedit, fire `value.set`.
pub(super) fn build_ime_commit_handler(
    value: Signal<String>,
    undo: UndoStackHandle,
) -> ElementImeCommitHandler {
    Arc::new(move |ev: &ImeCommitEvent, cx: &mut EventCtx| {
        let id = match cx.element_id() {
            Some(i) => i,
            None => return,
        };
        let state_rc = match cx.ime_state(id) {
            Some(s) => s,
            None => return,
        };

        let new_text: Option<String> = {
            let mut state = state_rc.borrow_mut();
            if ev.text.is_empty() {
                // Clear preedit only — no text committed
                state.preedit = None;
                None
            } else {
                debug_assert!(
                    state.text.is_char_boundary(state.caret),
                    "TextField caret not on char boundary before ime commit"
                );
                // Composition starts: selection collapses (policy R2).
                delete_selection(&mut state);
                let old_caret = state.caret;
                let new_caret = insert_text_at(&mut state.text, old_caret, &ev.text);
                state.caret = new_caret;
                state.preedit = None;
                record_edit(&undo, EditOp::Discrete, &state);
                reset_blink(&mut state);
                Some(state.text.clone())
            }
        };

        cx.stop_propagation();
        // Fire signal AFTER dropping RefMut
        if let Some(t) = new_text {
            value.set(t);
        }
    })
}

// ---------------------------------------------------------------------------
// Unit tests — selection model (Phase 10b.1)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod selection_tests {
    use super::*;

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
    fn paste_newline_stripping_keeps_other_chars() {
        let raw = "line1\nline2\r\nline3";
        let cleaned: String = raw.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        assert_eq!(cleaned, "line1line2line3");
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
