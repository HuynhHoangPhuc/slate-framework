//! TextArea event handler builders.
//!
//! The multi-line key handler shares the command-modifier shortcuts (copy/cut/
//! paste/undo/redo) and the byte-offset editing ops (←/→/Backspace/typing) with
//! TextField, and adds the line-relative motions (↑/↓/Home/End) plus Enter →
//! `\n`. Paste runs with `multiline = true` so newlines survive. The IME
//! handlers are TextArea-local copies of the TextField builders so the
//! single-line element's integration-tested IME path is never touched; the only
//! addition is clearing the sticky vertical-nav column (`desired_x`) on commit.
//!
//! Borrow discipline: every `value.set` fires after the `ImeState` `RefMut` has
//! been dropped.

use std::sync::Arc;

use slate_reactive::Signal;

use crate::event::{
    ElementImeCommitHandler, ElementImePreeditHandler, ElementKeyHandler, ElementTextInputHandler,
    EventCtx, ImeCommitEvent, ImePreeditEvent, Key, KeyEvent, NamedKey, TextInputEvent,
};
use crate::ime::Preedit;

use crate::elements::text_edit::grapheme::{
    insert_text_at, next_grapheme_boundary, prev_grapheme_boundary,
};
use crate::elements::text_edit::ops::{
    MotionDir, apply_motion, delete_selection, record_edit, reset_blink,
};
use crate::elements::text_edit::shortcuts;
use crate::elements::text_edit::undo::EditOp;

use super::nav;

/// Build the `on_key_down` handler for TextArea.
///
/// Order: command shortcuts (incl. multi-line paste) → IME guard → editing /
/// navigation arms. Enter is consumed here (inserts `\n`), unlike TextField.
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

        // Multi-line paste preserves newlines (`multiline = true`).
        if shortcuts::handle_command_shortcut(ev, cx, &state_rc, &value, true) {
            return;
        }

        // While IME composition is active, leave navigation to the IME.
        {
            let state = state_rc.borrow();
            if state.preedit.is_some() {
                return;
            }
        }

        let shift = ev.modifiers.shift;
        let new_text: Option<String> = match &ev.key {
            Key::Named(NamedKey::Enter) => {
                let t = {
                    let mut state = state_rc.borrow_mut();
                    nav::insert_newline(&mut state)
                };
                cx.stop_propagation();
                Some(t)
            }
            Key::Named(NamedKey::Backspace) => {
                let mut state = state_rc.borrow_mut();
                debug_assert!(
                    state.text.is_char_boundary(state.caret),
                    "TextArea caret not on char boundary"
                );
                state.desired_x = None;
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
            Key::Named(NamedKey::ArrowLeft) => {
                {
                    let mut state = state_rc.borrow_mut();
                    state.desired_x = None;
                    apply_motion(&mut state, MotionDir::Left, shift, |s| {
                        s.caret = prev_grapheme_boundary(&s.text, s.caret);
                    });
                    reset_blink(&mut state);
                    state.undo.mark_motion();
                }
                cx.stop_propagation();
                None
            }
            Key::Named(NamedKey::ArrowRight) => {
                {
                    let mut state = state_rc.borrow_mut();
                    state.desired_x = None;
                    apply_motion(&mut state, MotionDir::Right, shift, |s| {
                        s.caret = next_grapheme_boundary(&s.text, s.caret);
                    });
                    reset_blink(&mut state);
                    state.undo.mark_motion();
                }
                cx.stop_propagation();
                None
            }
            Key::Named(NamedKey::ArrowUp) | Key::Named(NamedKey::ArrowDown) => {
                let down = matches!(ev.key, Key::Named(NamedKey::ArrowDown));
                // Drop the shared borrow (the `Ref` from `borrow()`) before
                // taking `borrow_mut()` — under edition 2024 a `Ref` in an
                // `if let` scrutinee lives to the end of the body, so clone the
                // layout into a local first to avoid a double-borrow panic.
                let layout = state_rc.borrow().last_layout.clone();
                if let Some(layout) = layout {
                    let mut state = state_rc.borrow_mut();
                    nav::move_vertical(&mut state, &layout, down, shift);
                }
                cx.stop_propagation();
                None
            }
            Key::Named(NamedKey::Home) | Key::Named(NamedKey::End) => {
                let to_end = matches!(ev.key, Key::Named(NamedKey::End));
                let layout = state_rc.borrow().last_layout.clone();
                if let Some(layout) = layout {
                    let mut state = state_rc.borrow_mut();
                    nav::move_line_edge(&mut state, &layout, to_end, shift);
                }
                cx.stop_propagation();
                None
            }
            _ => None,
        };

        if let Some(t) = new_text {
            value.set(t);
        }
    })
}

/// Build the `on_text_input` handler for TextArea. Mirrors TextField but clears
/// the sticky vertical-nav column so a following ↑/↓ re-seeds it.
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
                "TextArea caret not on char boundary before text insert"
            );
            state.desired_x = None;
            let had_selection = state.selection_anchor.is_some_and(|a| a != state.caret);
            delete_selection(&mut state);
            let old_caret = state.caret;
            state.caret = insert_text_at(&mut state.text, old_caret, &ev.text);
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

/// Build the `on_ime_preedit` handler for TextArea (overlay only — no buffer
/// mutation, no signal write).
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

/// Build the `on_ime_commit` handler for TextArea. Inserts committed text at the
/// caret and clears the sticky vertical-nav column.
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
                state.preedit = None;
                None
            } else {
                debug_assert!(
                    state.text.is_char_boundary(state.caret),
                    "TextArea caret not on char boundary before ime commit"
                );
                state.desired_x = None;
                delete_selection(&mut state);
                let old_caret = state.caret;
                state.caret = insert_text_at(&mut state.text, old_caret, &ev.text);
                state.preedit = None;
                record_edit(&mut state, EditOp::Discrete);
                reset_blink(&mut state);
                Some(state.text.clone())
            }
        };

        cx.stop_propagation();
        if let Some(t) = new_text {
            value.set(t);
        }
    })
}
