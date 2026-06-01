//! Keyboard + click handlers for [`MenuList`](super::MenuList).
//!
//! One roving-focus key handler is shared by the list container and every row
//! (mirroring the S0 grid spike): it reads the active index from the caller's
//! `use_state` signal, computes the clamped next *enabled* row, writes it back,
//! and moves real focus there — so the first arrow also pulls focus off the
//! container onto a row, and a screen reader follows the active item.

use std::sync::Arc;

use slate_platform::{Key, NamedKey};

use crate::event::{ElementKeyHandler, EventCtx, Handlers, KeyEvent, MouseEvent, MouseHandler};
use crate::elements::roving::{end_enabled, step_enabled};
use crate::reactive::Signal;
use crate::types::ElementId;

use super::MenuActivate;

/// Build the roving navigation + activation key handler shared by the container
/// and all rows.
pub(super) fn roving_handler(
    active: Signal<usize>,
    row_ids: Arc<Vec<ElementId>>,
    disabled: Arc<Vec<bool>>,
    on_activate: MenuActivate,
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
                    on_activate(cur, cx);
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

/// Build the click handler for row `index`: activates it (if enabled) and stops
/// propagation so the click doesn't also reach the overlay's outside-click path.
pub(super) fn click_handler(index: usize, on_activate: MenuActivate) -> Handlers {
    let on_click: MouseHandler = Arc::new(move |_ev: &MouseEvent, cx: &mut EventCtx| {
        on_activate(index, cx);
        cx.stop_propagation();
    });
    Handlers {
        on_click: Some(on_click),
        ..Default::default()
    }
}
