//! TextField event handler builders.
//!
//! Each function returns a cloned-closure `Arc<dyn Fn + Send + Sync + 'static>`
//! ready to be stored in `KeyHandlers` / `ImeHandlers`. Closures capture
//! `Signal<String>` by clone so the handlers can fire `value.set()` without
//! holding any `RefMut` on `ImeState` (borrow discipline: always drop the
//! RefMut before calling `value.set`).

use std::sync::Arc;
use std::time::Instant;

use slate_reactive::Signal;
use slate_text::byte_at_pixel_x;

use crate::event::{
    ElementImeCommitHandler, ElementImePreeditHandler, ElementKeyHandler, ElementTextInputHandler,
    EventCtx, ImeCommitEvent, ImePreeditEvent, Key, KeyEvent, MouseEvent, MouseHandler, NamedKey,
    TextInputEvent,
};
use crate::ime::Preedit;

use super::grapheme::{insert_text_at, next_grapheme_boundary, prev_grapheme_boundary};
use super::{EditSnapshot, UndoStackHandle};

/// Capture the current edit state as a pre-mutation snapshot and push it onto
/// `undo`. Always-push coalesce stub (10a.7); 10b.3 replaces this with the
/// 4-trigger policy. Lock contention is non-existent in practice — handlers
/// always run on the main thread — so a poisoned mutex is the only failure
/// mode and is silently ignored to keep typing alive.
fn push_undo_snapshot(undo: &UndoStackHandle, text: &str, caret: usize, anchor: Option<usize>) {
    if let Ok(mut stack) = undo.lock() {
        stack.push(EditSnapshot {
            text: text.to_string(),
            caret,
            anchor,
            t: Instant::now(),
        });
    }
}

/// Build the `on_key_down` handler for TextField.
///
/// Handles: Backspace, ←, →, Home, End.
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

        // While IME composition is active, leave all navigation to the IME
        {
            let state = state_rc.borrow();
            if state.preedit.is_some() {
                return;
            }
        }

        let new_text: Option<String> = match &ev.key {
            Key::Named(NamedKey::Backspace) => {
                let mut state = state_rc.borrow_mut();
                debug_assert!(
                    state.text.is_char_boundary(state.caret),
                    "TextField caret not on char boundary"
                );
                let old_caret = state.caret;
                let new_caret = prev_grapheme_boundary(&state.text, old_caret);
                if new_caret < old_caret {
                    push_undo_snapshot(&undo, &state.text, old_caret, state.selection_anchor);
                    state.text.replace_range(new_caret..old_caret, "");
                    state.caret = new_caret;
                    cx.stop_propagation();
                    Some(state.text.clone())
                } else {
                    None
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                let mut state = state_rc.borrow_mut();
                state.caret = prev_grapheme_boundary(&state.text, state.caret);
                cx.stop_propagation();
                None
            }
            Key::Named(NamedKey::ArrowRight) => {
                let mut state = state_rc.borrow_mut();
                state.caret = next_grapheme_boundary(&state.text, state.caret);
                cx.stop_propagation();
                None
            }
            Key::Named(NamedKey::Home) => {
                let mut state = state_rc.borrow_mut();
                state.caret = 0;
                cx.stop_propagation();
                None
            }
            Key::Named(NamedKey::End) => {
                let mut state = state_rc.borrow_mut();
                state.caret = state.text.len();
                cx.stop_propagation();
                None
            }
            // Tab / Enter: bubble — do NOT stop propagation
            _ => None,
        };

        // Fire signal AFTER dropping RefMut to avoid borrow aliasing
        if let Some(t) = new_text {
            value.set(t);
        }
    })
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
            let old_caret = state.caret;
            push_undo_snapshot(&undo, &state.text, old_caret, state.selection_anchor);
            let new_caret = insert_text_at(&mut state.text, old_caret, &ev.text);
            state.caret = new_caret;
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
        drop(state);
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
                let old_caret = state.caret;
                push_undo_snapshot(&undo, &state.text, old_caret, state.selection_anchor);
                let new_caret = insert_text_at(&mut state.text, old_caret, &ev.text);
                state.caret = new_caret;
                state.preedit = None;
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
