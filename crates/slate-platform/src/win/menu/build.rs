//! Recursive `PlatformMenu` → `HMENU` construction.
//!
//! Actionable items get a Win32 command id allocated through a
//! [`MenuCommandMap`]; that id is what `WM_COMMAND` (menu bar) and
//! `TrackPopupMenu`'s `TPM_RETURNCMD` (context menu) report back, and the map
//! resolves it to the framework `MenuId`. Win32 packs the command id into the
//! low word of `WM_COMMAND`'s `wParam`, so ids are `u16` — far narrower than the
//! macOS backend's `tag` (which carries the full `MenuId`), hence the indirection
//! map instead of a direct cast.

use std::collections::HashMap;

use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateMenu, CreatePopupMenu, HMENU, MF_CHECKED, MF_GRAYED, MF_POPUP, MF_SEPARATOR,
    MF_STRING,
};
use windows::core::PCWSTR;

use super::super::to_wide;
use crate::{Key, Modifiers, PlatformMenu, PlatformMenuItem};

/// Lowest Win32 menu command id we hand out. Kept clear of `0` (which
/// `TPM_RETURNCMD` uses for "dismissed") and the low range conventionally used
/// by dialog/control ids.
const COMMAND_ID_BASE: u16 = 0x0100;

/// Allocates Win32 menu command ids and maps them back to framework `MenuId`s.
///
/// Ids are handed out monotonically and never reused within a process so a
/// stale entry can never mis-route to a different action — important once
/// multiple windows each own a live menu bar (Windows, unlike macOS, has no
/// single shared app menu bar). The `u16` space wraps back to the base on
/// exhaustion (65k distinct items in one process — far beyond any real menu),
/// trading a vanishingly unlikely stale-reuse for never panicking.
pub(crate) struct MenuCommandMap {
    next: u16,
    map: HashMap<u16, u64>,
}

impl MenuCommandMap {
    /// Empty map starting allocation at [`COMMAND_ID_BASE`].
    pub(crate) fn new() -> Self {
        Self {
            next: COMMAND_ID_BASE,
            map: HashMap::new(),
        }
    }

    /// Allocate a command id for `menu_id` and record the reverse mapping.
    fn alloc(&mut self, menu_id: u64) -> u16 {
        let cmd = self.next;
        self.next = if self.next == u16::MAX {
            COMMAND_ID_BASE
        } else {
            self.next + 1
        };
        self.map.insert(cmd, menu_id);
        cmd
    }

    /// Resolve a Win32 command id back to its framework `MenuId`.
    pub(crate) fn resolve(&self, cmd: u16) -> Option<u64> {
        self.map.get(&cmd).copied()
    }
}

/// Build the window menu bar (`CreateMenu`) from the model.
pub(crate) fn build_menu_bar(model: &PlatformMenu, map: &mut MenuCommandMap) -> HMENU {
    // SAFETY: CreateMenu is a pure handle constructor.
    let menu = unsafe { CreateMenu() }.expect("CreateMenu failed");
    append_items(menu, &model.items, map);
    menu
}

/// Build a standalone popup (context menu) from the model.
pub(crate) fn build_popup_menu(model: &PlatformMenu, map: &mut MenuCommandMap) -> HMENU {
    // SAFETY: CreatePopupMenu is a pure handle constructor.
    let menu = unsafe { CreatePopupMenu() }.expect("CreatePopupMenu failed");
    append_items(menu, &model.items, map);
    menu
}

fn append_items(into: HMENU, items: &[PlatformMenuItem], map: &mut MenuCommandMap) {
    for item in items {
        match item {
            PlatformMenuItem::Action {
                id,
                label,
                accelerator,
                enabled,
                checked,
            } => {
                let cmd = map.alloc(*id);
                let mut flags = MF_STRING;
                if !*enabled {
                    flags |= MF_GRAYED;
                }
                if *checked {
                    flags |= MF_CHECKED;
                }
                let text = menu_item_text(label, accelerator);
                let wide = to_wide(&text);
                // SAFETY: `into` is a live menu; `wide` is null-terminated UTF-16
                // and outlives the call (AppendMenuW copies the string).
                let _ = unsafe { AppendMenuW(into, flags, cmd as usize, PCWSTR(wide.as_ptr())) };
            }
            PlatformMenuItem::Submenu {
                label,
                enabled,
                items,
            } => {
                // SAFETY: pure handle constructor.
                let sub = unsafe { CreatePopupMenu() }.expect("CreatePopupMenu failed");
                append_items(sub, items, map);
                let mut flags = MF_POPUP | MF_STRING;
                if !*enabled {
                    flags |= MF_GRAYED;
                }
                let wide = to_wide(label);
                // For MF_POPUP, uIDNewItem is the submenu handle, not a command id.
                // SAFETY: `into`/`sub` are live menus; `wide` outlives the call.
                let _ = unsafe {
                    AppendMenuW(into, flags, sub.0 as usize, PCWSTR(wide.as_ptr()))
                };
            }
            PlatformMenuItem::Separator => {
                // SAFETY: MF_SEPARATOR ignores the id/text args.
                let _ = unsafe { AppendMenuW(into, MF_SEPARATOR, 0, PCWSTR::null()) };
            }
        }
    }
}

/// Compose a menu item's display string: `label` plus a tab-aligned accelerator
/// hint (`"Save\tCtrl+S"`) when one is present and textual. The `\t` makes Win32
/// right-align the accelerator, matching native menus.
fn menu_item_text(label: &str, accelerator: &Option<(Modifiers, Key)>) -> String {
    match accelerator {
        Some((mods, key)) => {
            let accel = accelerator_text(mods, key);
            if accel.is_empty() {
                label.to_string()
            } else {
                format!("{label}\t{accel}")
            }
        }
        None => label.to_string(),
    }
}

/// Render an accelerator as Win32 menu text (`"Ctrl+Shift+N"`).
///
/// Only `Key::Character` accelerators get textual hints — mirroring the macOS
/// backend, which likewise produces no key equivalent for named/unidentified
/// keys. This shows the hint only; firing the accelerator would require a
/// separate `HACCEL` table + `TranslateAccelerator` (deferred).
pub(crate) fn accelerator_text(mods: &Modifiers, key: &Key) -> String {
    let key_text = match key {
        Key::Character(c) => c.to_uppercase(),
        Key::Named(_) | Key::Unidentified => return String::new(),
    };
    let mut s = String::new();
    if mods.ctrl {
        s.push_str("Ctrl+");
    }
    if mods.alt {
        s.push_str("Alt+");
    }
    if mods.shift {
        s.push_str("Shift+");
    }
    if mods.meta {
        s.push_str("Win+");
    }
    s.push_str(&key_text);
    s
}

// Re-export so `menu_item_text` is unit-testable without exposing the whole
// AppendMenuW path (which needs a live HMENU).
#[cfg(test)]
pub(crate) fn menu_item_text_for_test(label: &str, accel: &Option<(Modifiers, Key)>) -> String {
    menu_item_text(label, accel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn character_accelerator_renders_with_modifiers() {
        let text = accelerator_text(
            &Modifiers {
                ctrl: true,
                ..Default::default()
            },
            &Key::Character("n".into()),
        );
        assert_eq!(text, "Ctrl+N");
    }

    #[test]
    fn modifier_order_is_ctrl_alt_shift_win() {
        let text = accelerator_text(
            &Modifiers {
                ctrl: true,
                alt: true,
                shift: true,
                meta: true,
            },
            &Key::Character("s".into()),
        );
        assert_eq!(text, "Ctrl+Alt+Shift+Win+S");
    }

    #[test]
    fn named_and_unidentified_keys_have_no_accel_text() {
        assert!(accelerator_text(&Modifiers::default(), &Key::Unidentified).is_empty());
    }

    #[test]
    fn item_text_appends_tab_separated_accel() {
        let text = menu_item_text_for_test(
            "Save",
            &Some((
                Modifiers {
                    ctrl: true,
                    ..Default::default()
                },
                Key::Character("s".into()),
            )),
        );
        assert_eq!(text, "Save\tCtrl+S");
    }

    #[test]
    fn item_text_without_accel_is_bare_label() {
        assert_eq!(menu_item_text_for_test("Quit", &None), "Quit");
    }

    #[test]
    fn command_map_allocates_unique_ids_and_resolves() {
        let mut map = MenuCommandMap::new();
        let a = map.alloc(10);
        let b = map.alloc(20);
        assert_ne!(a, b);
        assert_eq!(map.resolve(a), Some(10));
        assert_eq!(map.resolve(b), Some(20));
        assert_eq!(map.resolve(0xFFFF), None);
    }

    #[test]
    fn command_ids_start_at_reserved_base() {
        let mut map = MenuCommandMap::new();
        assert_eq!(map.alloc(1), COMMAND_ID_BASE);
    }
}
