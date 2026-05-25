//! Win32 IMM32 IME handlers: WM_IME_STARTCOMPOSITION, WM_IME_COMPOSITION,
//! WM_IME_ENDCOMPOSITION.

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::Input::Ime::{GCS_COMPSTR, GCS_RESULTSTR};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, WM_IME_COMPOSITION, WM_IME_ENDCOMPOSITION, WM_IME_STARTCOMPOSITION,
};

use super::WinWindowInner;
use super::super::dispatch_event;
use super::super::ime::{
    ImmContextGuard, find_target_converted_run, read_composition_attrs, read_composition_cursor,
    read_composition_string_utf8, set_composition_window, utf16_units_to_utf8_bytes,
};
use crate::Event;

impl WinWindowInner {
    /// Dispatch a WM_IME_* family message; returns `None` if not an IME msg.
    pub(super) fn dispatch_ime(
        &self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match msg {
            WM_IME_STARTCOMPOSITION => {
                self.composition_active.set(true);
                dispatch_event(Event::ImeEnabled { window: self.id });
                // Return 0 to suppress the IMM built-in preedit window; we
                // render the composition ourselves inside the focused element.
                Some(LRESULT(0))
            }
            WM_IME_COMPOSITION => Some(self.on_ime_composition(hwnd, msg, wparam, lparam)),
            WM_IME_ENDCOMPOSITION => {
                self.composition_active.set(false);
                // Clear any lingering preedit: a cancelled composition ends
                // without a final empty GCS_COMPSTR, and no element wires an
                // `on_ime_disabled` handler, so the overlay would otherwise stay
                // painted. A preceding commit already cleared it, making this a
                // no-op there.
                dispatch_event(Event::ImePreedit {
                    window: self.id,
                    text: String::new(),
                    cursor_byte_offset: 0,
                    selection: None,
                });
                dispatch_event(Event::ImeDisabled { window: self.id });
                // SAFETY: default proc must run for IMM cleanup.
                Some(unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
            }
            _ => None,
        }
    }

    fn on_ime_composition(
        &self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let flags = lparam.0 as u32;
        let Some(guard) = ImmContextGuard::acquire(hwnd) else {
            // SAFETY: default proc is always safe to call.
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        };
        let imc = guard.handle();
        let mut handled = false;

        if flags & GCS_RESULTSTR.0 != 0 {
            if let Some((text, _)) = read_composition_string_utf8(imc, GCS_RESULTSTR) {
                dispatch_event(Event::ImeCommit {
                    window: self.id,
                    text,
                });
            }
            handled = true;
        }
        if flags & GCS_COMPSTR.0 != 0 {
            if let Some((text, utf16)) = read_composition_string_utf8(imc, GCS_COMPSTR) {
                let cursor_u16 = read_composition_cursor(imc).unwrap_or(0);
                let cursor_byte_offset = utf16_units_to_utf8_bytes(&utf16, cursor_u16);
                let selection = read_composition_attrs(imc)
                    .and_then(|attrs| find_target_converted_run(&attrs, &utf16));
                dispatch_event(Event::ImePreedit {
                    window: self.id,
                    text,
                    cursor_byte_offset,
                    selection,
                });
            } else {
                // GCS_COMPSTR is flagged but the IMM reports a zero-length
                // string: the composition was deleted back to empty (the
                // user backspaced the last char) while the session stays
                // open. `read_composition_string_utf8` returns None for
                // length 0, so emit an explicit empty preedit — otherwise
                // the framework keeps painting the last non-empty
                // composition, which sticks on screen and blocks further
                // deletion.
                dispatch_event(Event::ImePreedit {
                    window: self.id,
                    text: String::new(),
                    cursor_byte_offset: 0,
                    selection: None,
                });
            }
            // Anchor the candidate window at the caret. The caret rect is
            // already in client-relative physical pixels (the element
            // paints the caret in scene/client space, and under
            // Per-Monitor-v2 awareness those are physical px), and
            // ImmSetCompositionWindow's COMPOSITIONFORM expects client
            // coordinates — so feed the rect through unchanged. (No
            // ScreenToClient: the rect was never in screen space, and
            // subtracting the window origin again would fling the
            // candidate window to a screen corner.)
            let weak = self.ime_delegate.borrow().clone();
            if let Some(weak) = weak
                && let Some(strong) = weak.upgrade()
                && let Some(rect) = strong.ime_caret_rect(self.id)
            {
                let client_rect = RECT {
                    left: rect.x,
                    top: rect.y,
                    right: rect.x + rect.width as i32,
                    bottom: rect.y + rect.height as i32,
                };
                set_composition_window(imc, client_rect);
            }
            handled = true;
        }
        drop(guard);
        if handled {
            LRESULT(0)
        } else {
            // SAFETY: default proc is always safe to call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
    }
}
