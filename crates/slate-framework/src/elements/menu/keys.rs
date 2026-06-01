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
use crate::reactive::Signal;
use crate::types::ElementId;

use super::MenuActivate;

/// First enabled index scanning from `start` in `dir` (+1 / -1), clamped at the
/// ends (no wrap). Returns `start` if no enabled row exists in that direction.
fn step_enabled(disabled: &[bool], start: usize, dir: isize) -> usize {
    let n = disabled.len() as isize;
    let mut i = start as isize;
    loop {
        let next = i + dir;
        if next < 0 || next >= n {
            // Fell off the end with no enabled row in this direction: hold at the
            // original position rather than landing on a disabled/edge row.
            return start;
        }
        i = next;
        if !disabled[i as usize] {
            return i as usize;
        }
    }
}

/// First / last enabled index (for Home / End). Falls back to `0`.
fn end_enabled(disabled: &[bool], from_start: bool) -> usize {
    let n = disabled.len();
    if from_start {
        (0..n).find(|&i| !disabled[i]).unwrap_or(0)
    } else {
        (0..n).rev().find(|&i| !disabled[i]).unwrap_or(0)
    }
}

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

#[cfg(test)]
mod tests {
    use super::{end_enabled, step_enabled};

    #[test]
    fn step_clamps_at_ends_when_all_enabled() {
        let d = [false, false, false];
        assert_eq!(step_enabled(&d, 0, -1), 0, "up at top stays");
        assert_eq!(step_enabled(&d, 2, 1), 2, "down at bottom stays");
        assert_eq!(step_enabled(&d, 1, 1), 2);
        assert_eq!(step_enabled(&d, 1, -1), 0);
    }

    #[test]
    fn step_skips_disabled_rows() {
        // index 1 disabled: from 0 down jumps to 2; from 2 up jumps to 0.
        let d = [false, true, false];
        assert_eq!(step_enabled(&d, 0, 1), 2);
        assert_eq!(step_enabled(&d, 2, -1), 0);
    }

    #[test]
    fn step_holds_when_no_enabled_neighbour() {
        // Only index 0 enabled: moving down finds nothing → stays at 0.
        let d = [false, true, true];
        assert_eq!(step_enabled(&d, 0, 1), 0);
    }

    #[test]
    fn home_end_pick_first_last_enabled() {
        let d = [true, false, false, true];
        assert_eq!(end_enabled(&d, true), 1, "Home skips leading disabled");
        assert_eq!(end_enabled(&d, false), 2, "End skips trailing disabled");
    }

    #[test]
    fn end_enabled_falls_back_when_all_disabled() {
        let d = [true, true];
        assert_eq!(end_enabled(&d, true), 0);
        assert_eq!(end_enabled(&d, false), 0);
    }
}
