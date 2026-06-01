//! Keyboard + click handlers for [`List`](super::List).
//!
//! Like [`MenuList`](crate::MenuList), one roving-focus key handler is shared by
//! the list container and every row: it reads the transient active index, steps
//! to the clamped next *enabled* row (Up/Down/Home/End), writes it back, and
//! moves **real** focus there so a screen reader follows the active item. Enter
//! / Space (and a row click) *commit*: they write the caller-owned selection
//! `Signal<Option<usize>>` and fire the optional activation callback.

use std::sync::Arc;

use slate_platform::{Key, NamedKey};

use crate::elements::roving::{end_enabled, step_enabled};
use crate::event::{ElementKeyHandler, EventCtx, Handlers, KeyEvent, KeyHandlers, MouseEvent, MouseHandler};
use crate::reactive::Signal;
use crate::types::ElementId;

use super::ListActivate;

/// Commit row `index`: write the selection signal and run the activation
/// callback. Shared by the Enter-key and click paths.
fn commit(
    index: usize,
    selected: &Signal<Option<usize>>,
    on_activate: &ListActivate,
    cx: &mut EventCtx,
) {
    selected.set(Some(index));
    on_activate(index, cx);
}

/// Build the roving navigation + commit key handler shared by the container and
/// all rows.
pub(super) fn roving_handler(
    active: Signal<usize>,
    selected: Signal<Option<usize>>,
    row_ids: Arc<Vec<ElementId>>,
    disabled: Arc<Vec<bool>>,
    on_activate: ListActivate,
) -> ElementKeyHandler {
    Arc::new(move |ev: &KeyEvent, cx: &mut EventCtx| {
        if ev.is_repeat && matches!(ev.key, Key::Named(NamedKey::Enter | NamedKey::Space)) {
            return;
        }
        let cur = active.get().min(row_ids.len().saturating_sub(1));
        let target = match ev.key {
            Key::Named(NamedKey::ArrowDown) => step_enabled(&disabled, cur, 1),
            Key::Named(NamedKey::ArrowUp) => step_enabled(&disabled, cur, -1),
            Key::Named(NamedKey::Home) => end_enabled(&disabled, true),
            Key::Named(NamedKey::End) => end_enabled(&disabled, false),
            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Space) => {
                if !disabled.get(cur).copied().unwrap_or(true) {
                    commit(cur, &selected, &on_activate, cx);
                }
                cx.stop_propagation();
                return;
            }
            _ => return,
        };
        if target != cur {
            active.set(target);
        }
        if let Some(&id) = row_ids.get(target) {
            cx.request_focus(id);
        }
        cx.stop_propagation();
    })
}

/// Wrap a key handler as the `on_key_down`-only [`KeyHandlers`].
pub(super) fn key_handlers(h: ElementKeyHandler) -> KeyHandlers {
    KeyHandlers { on_key_down: Some(h), ..Default::default() }
}

/// Build the click handler for row `index`: commits it (if enabled) and stops
/// propagation so the click doesn't also reach an overlay's outside-click path.
pub(super) fn click_handler(
    index: usize,
    selected: Signal<Option<usize>>,
    on_activate: ListActivate,
) -> Handlers {
    let on_click: MouseHandler = Arc::new(move |_ev: &MouseEvent, cx: &mut EventCtx| {
        commit(index, &selected, &on_activate, cx);
        cx.stop_propagation();
    });
    Handlers { on_click: Some(on_click), ..Default::default() }
}
