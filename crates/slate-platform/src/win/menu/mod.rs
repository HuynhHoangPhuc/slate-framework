//! Windows native menu backend: lowers a [`PlatformMenu`] into `HMENU` for the
//! per-window menu bar (`SetMenu`) and native context menus (`TrackPopupMenu`),
//! and routes selections back to the framework as [`Event::MenuActivated`].
//!
//! Split by concern: [`build`] constructs the native tree + allocates command
//! ids, [`command`] handles `WM_COMMAND` (menu-bar picks). The menu-bar command
//! map is a thread-local because `WM_COMMAND` arrives long after the menu is
//! built, in a separate message-pump turn — so the id→`MenuId` mapping must
//! outlive the build call (unlike the context-menu path, which resolves inline
//! from a stack-local map and re-injects via a posted message).
//!
//! [`Event`]: crate::Event

mod build;
mod command;

use std::cell::RefCell;
use std::collections::HashMap;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyMenu, DrawMenuBar, GetMenu, SetMenu, TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD,
    TPM_TOPALIGN, TrackPopupMenu,
};

pub(crate) use command::dispatch_command;

use self::build::MenuCommandMap;
use super::WM_APP_MENU;
use crate::PlatformMenu;

// The context-menu path round-trips a full `MenuId` (`u64`) through a
// `WM_APP_MENU` `wParam` (`usize`). That is lossless only where `usize` is at
// least 64-bit — true on every supported Windows target (x64 / arm64), but make
// the invariant a hard compile error rather than a silent truncation if a
// 32-bit target is ever added.
const _: () = assert!(usize::BITS >= 64, "menu id round-trip needs 64-bit usize");

thread_local! {
    /// Per-window menu-bar command-id → `MenuId` maps, keyed by raw `HWND`.
    ///
    /// Each window's map is *replaced* on its own `set_menu_bar` (so it stays
    /// bounded to the current bar — no per-rebuild growth) and dropped when the
    /// window is destroyed ([`forget_window`]). Per-window keying lets multiple
    /// live menu bars route independently (Windows, unlike macOS, has no single
    /// shared app menu bar) and sets up per-window menu ownership.
    static MENU_COMMANDS: RefCell<HashMap<isize, MenuCommandMap>> = RefCell::new(HashMap::new());
}

/// Resolve a `WM_COMMAND` command id against `hwnd`'s menu-bar map.
fn resolve_menu_command(hwnd: HWND, cmd: u16) -> Option<u64> {
    MENU_COMMANDS.with(|c| c.borrow().get(&(hwnd.0 as isize))?.resolve(cmd))
}

/// Drop a window's menu-bar command map. Called from `WM_DESTROY` so a closed
/// window's ids don't linger for the process lifetime.
pub(crate) fn forget_window(hwnd: HWND) {
    MENU_COMMANDS.with(|c| {
        c.borrow_mut().remove(&(hwnd.0 as isize));
    });
}

/// Install `model` as `hwnd`'s native menu bar (`SetMenu` + `DrawMenuBar`).
///
/// Replaces and destroys any previously-installed bar so repeated `set_menu`
/// calls (e.g. toggling a checked item) don't leak `HMENU`s, and replaces this
/// window's command map so its id set stays bounded to the current bar.
pub(crate) fn set_menu_bar(hwnd: HWND, model: &PlatformMenu) {
    let new_menu = MENU_COMMANDS.with(|c| {
        let mut maps = c.borrow_mut();
        let map = maps.entry(hwnd.0 as isize).or_insert_with(MenuCommandMap::new);
        // Fresh map: drop the previous bar's ids rather than accumulate them.
        *map = MenuCommandMap::new();
        build::build_menu_bar(model, map)
    });
    // SAFETY: hwnd is a live window; GetMenu/SetMenu/DrawMenuBar/DestroyMenu take
    // valid handles. We destroy the previous bar only after SetMenu detaches it.
    unsafe {
        let old = GetMenu(hwnd);
        let _ = SetMenu(hwnd, Some(new_menu));
        let _ = DrawMenuBar(hwnd);
        if !old.is_invalid() && old.0 != new_menu.0 {
            let _ = DestroyMenu(old);
        }
    }
}

/// Pop up `model` as a native context menu at `client_pos` (physical client
/// pixels, top-left origin) over `hwnd`.
///
/// Uses `TPM_RETURNCMD | TPM_NONOTIFY` so the selection is returned inline and
/// **no** `WM_COMMAND` is sent: the framework's event handler is typically still
/// on the stack (this is called from a right-click handler) holding the dispatch
/// borrow, so an inline dispatch would be dropped. The chosen id is re-injected
/// via `PostMessageW(WM_APP_MENU)`, dispatched on the next pump turn once the
/// borrow has unwound — the same deferral the macOS backend uses.
pub(crate) fn pop_up_context_menu(hwnd: HWND, model: &PlatformMenu, client_pos: (i32, i32)) {
    let mut local = MenuCommandMap::new();
    let menu = build::build_popup_menu(model, &mut local);

    let mut pt = POINT {
        x: client_pos.0,
        y: client_pos.1,
    };
    // SAFETY: hwnd is valid; pt is a valid in/out POINT. TrackPopupMenu wants
    // screen coordinates.
    unsafe {
        let _ = ClientToScreen(hwnd, &mut pt);
    }

    // SAFETY: `menu` is a live popup; hwnd is the valid owner window. With
    // TPM_RETURNCMD the BOOL return carries the chosen command id (0 = dismissed).
    let chosen = unsafe {
        TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_LEFTALIGN | TPM_TOPALIGN,
            pt.x,
            pt.y,
            None,
            hwnd,
            None,
        )
    };
    // SAFETY: popup menus are not attached to a window, so we own the handle and
    // must free it ourselves.
    unsafe {
        let _ = DestroyMenu(menu);
    }

    let cmd = chosen.0 as u16;
    if cmd != 0
        && let Some(id) = local.resolve(cmd)
    {
        // SAFETY: PostMessageW queues the message; hwnd is valid. wParam
        // carries the full MenuId (usize is 64-bit on the x64 target).
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(hwnd),
                WM_APP_MENU,
                windows::Win32::Foundation::WPARAM(id as usize),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
    }
}
