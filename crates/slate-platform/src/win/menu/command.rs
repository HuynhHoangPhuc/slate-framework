//! `WM_COMMAND` routing for native menu-bar selections.
//!
//! A menu-bar pick arrives asynchronously through the normal message pump (not
//! re-entrantly, unlike the `TrackPopupMenu` context-menu path), so it dispatches
//! inline here. The command id in `LOWORD(wParam)` is resolved against the
//! menu-bar command map to the framework `MenuId`, then posted as
//! [`Event::MenuActivated`] crediting the owning window (`self.id` at the call
//! site — Windows menus are per-window, satisfying the hwnd→`WindowId` mapping
//! for free).

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::WM_COMMAND;

use super::super::dispatch_event;
use crate::{Event, WindowId};

/// Handle `WM_COMMAND` if it is a menu selection; `None` otherwise so the master
/// router falls through to `DefWindowProcW`.
pub(crate) fn dispatch_command(
    window: WindowId,
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> Option<LRESULT> {
    if msg != WM_COMMAND {
        return None;
    }
    // Control notifications carry the control's HWND in lParam; menu (and
    // accelerator) commands set it to 0. We install no child controls and no
    // accelerator table, so a zero lParam here is always a menu pick.
    if lparam.0 != 0 {
        return None;
    }
    let cmd = (wparam.0 & 0xFFFF) as u16;
    match super::resolve_menu_command(hwnd, cmd) {
        Some(id) => {
            dispatch_event(Event::MenuActivated { window, id });
            Some(LRESULT(0))
        }
        // Unknown menu id (e.g. a stale bar replaced mid-flight): swallow it so
        // DefWindowProc does not beep, but do not fabricate an activation.
        None => Some(LRESULT(0)),
    }
}
