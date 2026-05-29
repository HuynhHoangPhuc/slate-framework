//! Shared command-modifier shortcuts (copy / cut / paste / undo / redo).
//!
//! Extracted verbatim from the TextField key handler so the single-line and
//! multi-line elements share one clipboard + undo path. The only behavioral
//! axis is `multiline`: it is forwarded to [`clipboard::clean_paste`] so a
//! `TextField` strips newlines from a paste while a `TextArea` keeps them
//! (CRLF→LF normalized). Any consumed shortcut that moves the caret also clears
//! the sticky vertical-nav column (`desired_x`) so a later ↑/↓ re-seeds it.
//!
//! Borrow discipline: the `ImeState` `RefMut` is always dropped before
//! `value.set` fires (the signal write can re-enter layout).

use std::cell::RefCell;
use std::rc::Rc;

use slate_reactive::Signal;

use crate::event::{EventCtx, KeyCode, KeyEvent, is_command_modifier};
use crate::ime::ImeState;

use super::clipboard;
use super::ops::apply_snapshot;

/// Handle a command-modifier shortcut on `ev`. Returns `true` when the event
/// was consumed (the caller must then stop and not run navigation/edit arms).
///
/// Runs BEFORE the IME guard so Paste can abort an active composition and
/// Undo/Redo work even if a stale preedit persists. `multiline` selects the
/// paste newline policy.
pub(crate) fn handle_command_shortcut(
    ev: &KeyEvent,
    cx: &mut EventCtx,
    state_rc: &Rc<RefCell<ImeState>>,
    value: &Signal<String>,
    multiline: bool,
) -> bool {
    if !is_command_modifier(&ev.modifiers) || ev.modifiers.alt {
        return false;
    }
    let shift = ev.modifiers.shift;
    match ev.code {
        KeyCode::KeyC => {
            let state = state_rc.borrow();
            if state.preedit.is_some() {
                return true;
            }
            if let Some(payload) = clipboard::selected_text(&state) {
                drop(state);
                slate_platform::clipboard::set_text(&payload);
            }
            cx.stop_propagation();
            true
        }
        KeyCode::KeyX => {
            let payload = {
                let state = state_rc.borrow();
                if state.preedit.is_some() {
                    return true;
                }
                clipboard::selected_text(&state)
            };
            if let Some(text) = payload {
                slate_platform::clipboard::set_text(&text);
                let new_text = {
                    let mut state = state_rc.borrow_mut();
                    state.desired_x = None;
                    clipboard::apply_cut(&mut state)
                };
                value.set(new_text);
            }
            cx.stop_propagation();
            true
        }
        KeyCode::KeyV => {
            let pasted = match slate_platform::clipboard::get_text() {
                Some(t) => t,
                None => {
                    cx.stop_propagation();
                    return true;
                }
            };
            let cleaned = clipboard::clean_paste(&pasted, multiline);
            if cleaned.is_empty() {
                cx.stop_propagation();
                return true;
            }
            let new_text = {
                let mut state = state_rc.borrow_mut();
                state.desired_x = None;
                clipboard::apply_paste(&mut state, &cleaned)
            };
            cx.stop_propagation();
            value.set(new_text);
            true
        }
        // Undo: Cmd/Ctrl+Z (without shift). Redo on macOS: Cmd+Shift+Z.
        KeyCode::KeyZ => {
            #[cfg(target_os = "macos")]
            let is_redo = shift;
            #[cfg(not(target_os = "macos"))]
            let is_redo = false;
            let _ = shift; // used on macOS only
            let restored = {
                let mut state = state_rc.borrow_mut();
                let snap = if is_redo {
                    state.undo.redo()
                } else {
                    state.undo.undo()
                };
                if let Some(ref s) = snap {
                    apply_snapshot(&mut state, s);
                    state.desired_x = None;
                }
                snap
            };
            if let Some(snap) = restored {
                value.set(snap.text);
            }
            cx.stop_propagation();
            true
        }
        // Redo on Windows/Linux: Ctrl+Y (without shift).
        #[cfg(not(target_os = "macos"))]
        KeyCode::KeyY if !shift => {
            let restored = {
                let mut state = state_rc.borrow_mut();
                let snap = state.undo.redo();
                if let Some(ref s) = snap {
                    apply_snapshot(&mut state, s);
                    state.desired_x = None;
                }
                snap
            };
            if let Some(snap) = restored {
                value.set(snap.text);
            }
            cx.stop_propagation();
            true
        }
        _ => false,
    }
}
