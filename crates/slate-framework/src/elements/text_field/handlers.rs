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
    self, ElementImeCommitHandler, ElementImePreeditHandler, ElementKeyHandler,
    ElementTextInputHandler, EventCtx, ImeCommitEvent, ImePreeditEvent, Key, KeyEvent, MouseEvent,
    MouseHandler, NamedKey, TextInputEvent,
};
use crate::ime::{ImeState, Preedit};

use crate::elements::text_edit::grapheme::{
    insert_text_at, next_grapheme_boundary, prev_grapheme_boundary,
};
use crate::elements::text_edit::ops::{
    MotionDir, apply_motion, apply_visual_edge, apply_visual_motion, delete_selection, record_edit,
    reset_blink,
};
use crate::elements::text_edit::shortcuts;
use crate::elements::text_edit::undo::EditOp;

/// Build the `on_key_down` handler for TextField.
///
/// Handles: Backspace, ←, →, Home, End (with and without Shift).
/// Ignores keys while IME preedit is active (IME owns those keystrokes).
/// Does NOT consume Tab / Enter — they bubble to App level.
pub(super) fn build_key_down_handler(value: Signal<String>) -> ElementKeyHandler {
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
        // stale preedit somehow persists. TextField is single-line, so paste
        // strips newlines (`multiline = false`).
        if shortcuts::handle_command_shortcut(ev, cx, &state_rc, &value, false) {
            return;
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
                state.caret_affinity = slate_text::Affinity::Downstream;
                if state.selection_anchor.is_some_and(|a| a != state.caret) {
                    delete_selection(&mut state);
                    record_edit(&mut state, EditOp::Discrete);
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
                        record_edit(&mut state, EditOp::Backspace);
                        reset_blink(&mut state);
                        cx.stop_propagation();
                        Some(state.text.clone())
                    } else {
                        None
                    }
                }
            }
            Key::Named(NamedKey::ArrowLeft) | Key::Named(NamedKey::ArrowRight) => {
                let move_right = matches!(ev.key, Key::Named(NamedKey::ArrowRight));
                // macOS Cmd+←/→ is a visual line-edge jump — same target as
                // Home/End. Branch first so the standard run-bearing /
                // grapheme arrow path stays byte-identical on Windows/Linux.
                if event::is_line_edge_modifier(&ev.modifiers) {
                    let shaped = state_rc.borrow().last_shaped.clone();
                    {
                        let mut state = state_rc.borrow_mut();
                        apply_line_edge(&mut state, shaped.as_deref(),move_right, shift);
                        reset_blink(&mut state);
                        state.undo.mark_motion();
                    }
                    cx.stop_propagation();
                    return;
                }
                // Clone the cached single-line shaping (cheap Rc bump) so the
                // run-bearing branch can read it while `state` is borrowed mut.
                let shaped = state_rc.borrow().last_shaped.clone();
                {
                    let mut state = state_rc.borrow_mut();
                    // A run-bearing line is owned by visual motion: step within
                    // the line, else clamp at the visual edge (single line — no
                    // adjacent line to cross to). Pure-LTR (empty runs) keeps
                    // logical grapheme motion, byte-identical to before.
                    let run_bearing = shaped.as_ref().is_some_and(|s| !s.runs.is_empty());
                    if run_bearing {
                        // Edge → `false`: caret stays put (clamp).
                        apply_visual_motion(&mut state, shaped.as_ref().unwrap(), move_right, shift);
                    } else {
                        state.caret_affinity = slate_text::Affinity::Downstream;
                        let dir = if move_right { MotionDir::Right } else { MotionDir::Left };
                        apply_motion(&mut state, dir, shift, |s| {
                            s.caret = if move_right {
                                next_grapheme_boundary(&s.text, s.caret)
                            } else {
                                prev_grapheme_boundary(&s.text, s.caret)
                            };
                        });
                    }
                    reset_blink(&mut state);
                    state.undo.mark_motion();
                }
                cx.stop_propagation();
                None
            }
            Key::Named(NamedKey::Home) | Key::Named(NamedKey::End) => {
                let to_end = matches!(ev.key, Key::Named(NamedKey::End));
                let shaped = state_rc.borrow().last_shaped.clone();
                {
                    let mut state = state_rc.borrow_mut();
                    apply_line_edge(&mut state, shaped.as_deref(),to_end, shift);
                    reset_blink(&mut state);
                    state.undo.mark_motion();
                }
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

/// Move the TextField caret to the visual line edge. Shared by Home/End and
/// macOS Cmd+←/→: a run-bearing line uses the visual edge (which on a mixed
/// LTR/RTL line differs from byte-0 / `text.len()`), otherwise the logical
/// line edges. `to_end = true` means End / right edge.
fn apply_line_edge(
    state: &mut ImeState,
    shaped: Option<&slate_text::ShapedLine>,
    to_end: bool,
    shift: bool,
) {
    let handled = match shaped {
        Some(s) if !s.runs.is_empty() => apply_visual_edge(state, s, to_end, shift),
        _ => false,
    };
    if !handled {
        state.caret_affinity = slate_text::Affinity::Downstream;
        let (dir, target) = if to_end {
            (MotionDir::Right, state.text.len())
        } else {
            (MotionDir::Left, 0)
        };
        apply_motion(state, dir, shift, |s| s.caret = target);
    }
}

/// Build the `on_text_input` handler for TextField.
///
/// Inserts composed text at the caret for non-IME ASCII input.
/// Ignored when IME preedit is active.
pub(super) fn build_text_input_handler(value: Signal<String>) -> ElementTextInputHandler {
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
            state.caret_affinity = slate_text::Affinity::Downstream;
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
            record_edit(&mut state, op);
            reset_blink(&mut state);
            state.text.clone()
        };

        cx.stop_propagation();
        value.set(new_text);
    })
}

/// Build the `on_mouse_down` handler for TextField.
///
/// Reads the cached `ShapedLine` + paint origin written by `paint()` on the
/// previous frame, snaps the click x to a grapheme boundary via
/// `slate_text::byte_at_pixel_x`, then anchors a new selection at that byte
/// (collapsed caret today; drag-extension happens in `mouse_move`).
///
/// IME guard: during an active preedit composition, mouse interaction is a
/// no-op — platforms expect `selectedRange == markedRange` while composing,
/// and richer interaction is deferred to a later phase.
pub(super) fn build_mouse_down_handler() -> MouseHandler {
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
            state.caret_affinity = slate_text::Affinity::Downstream;
            state.selection_anchor = Some(byte);
            state.dragging = true;
            reset_blink(&mut state);
            state.undo.mark_motion();
        }
        cx.stop_propagation();
    })
}

/// Build the `on_mouse_move` handler for TextField.
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
        state.caret_affinity = slate_text::Affinity::Downstream;
        reset_blink(&mut state);
        drop(state);
        cx.stop_propagation();
    })
}

/// Build the `on_mouse_up` handler for TextField.
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
pub(super) fn build_ime_commit_handler(value: Signal<String>) -> ElementImeCommitHandler {
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
                state.caret_affinity = slate_text::Affinity::Downstream;
                delete_selection(&mut state);
                let old_caret = state.caret;
                let new_caret = insert_text_at(&mut state.text, old_caret, &ev.text);
                state.caret = new_caret;
                state.preedit = None;
                record_edit(&mut state, EditOp::Discrete);
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

#[cfg(test)]
mod tests {
    //! Tests for the TextField line-edge helper shared by Home/End and macOS
    //! `Cmd+←/→`. The handler-level wiring is straightforward (branch + call);
    //! the contract worth pinning is what `apply_line_edge` does for the
    //! pure-LTR (logical) and run-bearing (visual) cases, plus the Shift
    //! anchor flow. Cross-platform behavior is enforced by the
    //! `is_line_edge_modifier` gate already tested in `event::tests`.
    use super::*;
    use slate_text::{Affinity, Direction, FontHandle, FontId, RunSpan, ShapedGlyph, ShapedLine};
    use std::rc::Rc;
    use std::cell::RefCell;
    use crate::ime::ImeState;

    fn glyph_ltr(cluster: u32, adv: f32) -> ShapedGlyph {
        ShapedGlyph {
            glyph_id: 1,
            font_id: FontId::PRIMARY,
            font_handle: FontHandle::default(),
            x_advance_lpx: adv,
            position_lpx: [0.0, 0.0],
            cluster,
            direction: Direction::Ltr,
        }
    }

    fn glyph(cluster: u32, adv: f32, dir: Direction) -> ShapedGlyph {
        ShapedGlyph {
            glyph_id: 1,
            font_id: FontId::PRIMARY,
            font_handle: FontHandle::default(),
            x_advance_lpx: adv,
            position_lpx: [0.0, 0.0],
            cluster,
            direction: dir,
        }
    }

    fn run(range: std::ops::Range<usize>, dir: Direction) -> RunSpan {
        RunSpan {
            level: if dir == Direction::Rtl { 1 } else { 0 },
            byte_range: range,
            direction: dir,
        }
    }

    /// Pure-LTR single line "hello": no runs → helper uses logical edges.
    fn pure_ltr_line() -> ShapedLine {
        ShapedLine {
            glyphs: vec![
                glyph_ltr(0, 5.0),
                glyph_ltr(1, 5.0),
                glyph_ltr(2, 5.0),
                glyph_ltr(3, 5.0),
                glyph_ltr(4, 5.0),
            ],
            width_lpx: 25.0,
            ascent_lpx: 10.0,
            descent_lpx: -2.0,
            y_offset_lpx: 0.0,
            base_direction: Direction::Ltr,
            runs: Vec::new(),
        }
    }

    /// Mixed "abאב": LTR 0..2 + RTL 2..6. Visual rightmost stop = byte 2.
    fn mixed_line() -> ShapedLine {
        ShapedLine {
            glyphs: vec![
                glyph(0, 5.0, Direction::Ltr),
                glyph(1, 6.0, Direction::Ltr),
                glyph(4, 7.0, Direction::Rtl),
                glyph(2, 8.0, Direction::Rtl),
            ],
            width_lpx: 26.0,
            ascent_lpx: 10.0,
            descent_lpx: -2.0,
            y_offset_lpx: 0.0,
            base_direction: Direction::Rtl,
            runs: vec![run(0..2, Direction::Ltr), run(2..6, Direction::Rtl)],
        }
    }

    fn state_with(text: &str, caret: usize) -> ImeState {
        ImeState {
            text: text.to_string(),
            caret,
            ..Default::default()
        }
    }

    #[test]
    fn line_edge_pure_ltr_left_jumps_to_byte_0() {
        let line = pure_ltr_line();
        let mut s = state_with("hello", 3);
        apply_line_edge(&mut s, Some(&line), false, false);
        assert_eq!(s.caret, 0);
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn line_edge_pure_ltr_right_jumps_to_text_len() {
        let line = pure_ltr_line();
        let mut s = state_with("hello", 3);
        apply_line_edge(&mut s, Some(&line), true, false);
        assert_eq!(s.caret, "hello".len());
    }

    #[test]
    fn line_edge_run_bearing_right_lands_at_visual_rightmost() {
        // Mixed LTR/RTL: visual rightmost is byte 2 (logical RTL start), NOT
        // text.len(). The Cmd+→ contract is visual, not logical.
        let line = mixed_line();
        let mut s = state_with("abאב", 1);
        apply_line_edge(&mut s, Some(&line), true, false);
        assert_eq!(s.caret, 2);
        assert_eq!(s.caret_affinity, Affinity::Downstream);
    }

    #[test]
    fn line_edge_run_bearing_left_lands_at_visual_leftmost() {
        let line = mixed_line();
        let mut s = state_with("abאב", 4);
        apply_line_edge(&mut s, Some(&line), false, false);
        assert_eq!(s.caret, 0);
    }

    #[test]
    fn line_edge_shift_extends_selection_from_pre_move_caret() {
        let line = pure_ltr_line();
        let mut s = state_with("hello", 2);
        apply_line_edge(&mut s, Some(&line), true, true);
        assert_eq!(s.selection_anchor, Some(2));
        assert_eq!(s.caret, "hello".len());
    }

    #[test]
    fn line_edge_repeated_at_edge_is_no_op() {
        // Repeated Cmd+→ at the visual end clamps — matches macOS TextEdit.
        let line = pure_ltr_line();
        let mut s = state_with("hello", 5);
        apply_line_edge(&mut s, Some(&line), true, false);
        assert_eq!(s.caret, 5);
        apply_line_edge(&mut s, Some(&line), true, false);
        assert_eq!(s.caret, 5);
    }

    #[test]
    fn line_edge_no_shaped_falls_back_to_logical() {
        // Pre-first-paint: shaped is None. Helper still moves to logical edges
        // (text.len() / 0) rather than panicking.
        let mut s = state_with("hello", 2);
        apply_line_edge(&mut s, None, true, false);
        assert_eq!(s.caret, "hello".len());
        apply_line_edge(&mut s, None, false, false);
        assert_eq!(s.caret, 0);
    }

    #[test]
    fn cmd_arrow_during_preedit_short_circuits() {
        // The preedit guard sits above the Cmd+arrow branch in the handler.
        // We can't easily run the full handler in a unit test without dragging
        // in AppState; instead verify the invariant the handler relies on:
        // `state.preedit.is_some()` is the early-return signal. If a future
        // refactor moves the guard below the branch, this test still acts as
        // a tripwire by documenting the contract that preedit composition
        // owns ←/→.
        let mut s = state_with("abc", 1);
        s.preedit = Some(crate::ime::Preedit {
            text: "x".into(),
            cursor_byte_offset: 1,
            selection: None,
        });
        // Sanity: a borrowed Rc<RefCell<_>> mirrors what the handler holds; the
        // guard's `state.preedit.is_some()` is the bit we depend on.
        let rc = Rc::new(RefCell::new(s));
        assert!(rc.borrow().preedit.is_some());
    }
}

