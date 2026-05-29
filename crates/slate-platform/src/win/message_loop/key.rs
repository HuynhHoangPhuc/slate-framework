//! Win32 keyboard and text handlers: WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
//! WM_SYSKEYUP, WM_CHAR, WM_UNICHAR.

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, WM_CHAR, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_UNICHAR,
};

use super::super::{dispatch_event, keymap};
use super::WinWindowInner;
use crate::{Event, Key};

impl WinWindowInner {
    /// Dispatch a keyboard/text Win32 message; returns `None` if not a key msg.
    pub(super) fn dispatch_key(
        &self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match msg {
            WM_KEYDOWN | WM_SYSKEYDOWN => Some(self.on_key_down(hwnd, msg, wparam, lparam)),
            WM_KEYUP | WM_SYSKEYUP => Some(self.on_key_up(hwnd, msg, wparam, lparam)),
            WM_CHAR => Some(self.on_char(wparam)),
            WM_UNICHAR => Some(self.on_unichar(wparam)),
            _ => None,
        }
    }

    fn on_key_down(&self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let vk = wparam.0 as u32;
        let lp = lparam.0 as u32;
        let scancode = (lp >> 16) & 0xFF;
        let extended = (lp >> 24) & 0x01 != 0;
        let is_repeat = (lp >> 30) & 0x01 != 0;
        let code = keymap::decode_keycode(vk, scancode, extended);
        let key = keymap::vk_to_named_key(vk)
            .map(Key::Named)
            .or_else(|| keymap::vk_to_character(vk, scancode).map(Key::Character))
            .unwrap_or(Key::Unidentified);
        let modifiers = keymap::read_modifiers();
        dispatch_event(Event::KeyDown {
            window: self.id,
            code,
            key,
            modifiers,
            is_repeat,
        });
        if msg == WM_SYSKEYDOWN {
            // Fall through so Alt-menu activation (Alt+F, Alt+Space, Alt+F4)
            // continues to work via the system's default handling.
            // SAFETY: default proc is always safe to call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        } else {
            LRESULT(0)
        }
    }

    fn on_key_up(&self, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let vk = wparam.0 as u32;
        let lp = lparam.0 as u32;
        let scancode = (lp >> 16) & 0xFF;
        let extended = (lp >> 24) & 0x01 != 0;
        let code = keymap::decode_keycode(vk, scancode, extended);
        let key = keymap::vk_to_named_key(vk)
            .map(Key::Named)
            .or_else(|| keymap::vk_to_character(vk, scancode).map(Key::Character))
            .unwrap_or(Key::Unidentified);
        let modifiers = keymap::read_modifiers();
        // Drop any orphan high surrogate left by a stalled WM_CHAR sequence.
        self.pending_high_surrogate.set(None);
        dispatch_event(Event::KeyUp {
            window: self.id,
            code,
            key,
            modifiers,
        });
        if msg == WM_SYSKEYUP {
            // SAFETY: default proc is always safe to call.
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        } else {
            LRESULT(0)
        }
    }

    fn on_char(&self, wparam: WPARAM) -> LRESULT {
        let code_unit = wparam.0 as u16;
        // Filter ASCII control range except Tab (0x09) and CR (0x0D, normalized to LF).
        if (code_unit < 0x20 && code_unit != 0x09 && code_unit != 0x0D) || code_unit == 0x7F {
            return LRESULT(0);
        }
        let text: Option<String> = if (0xD800..=0xDBFF).contains(&code_unit) {
            // High surrogate — stash for join on next WM_CHAR.
            self.pending_high_surrogate.set(Some(code_unit));
            None
        } else if (0xDC00..=0xDFFF).contains(&code_unit) {
            // Low surrogate — join with pending high.
            if let Some(high) = self.pending_high_surrogate.take() {
                let cp = 0x10000 + (((high - 0xD800) as u32) << 10) + ((code_unit - 0xDC00) as u32);
                char::from_u32(cp).map(|c| c.to_string())
            } else {
                // Orphan low — drop silently.
                None
            }
        } else {
            // BMP code unit; normalize CR → LF for text input.
            let unit = if code_unit == 0x0D { 0x0A } else { code_unit };
            String::from_utf16(&[unit]).ok().filter(|s| !s.is_empty())
        };
        if let Some(text) = text {
            dispatch_event(Event::TextInput {
                window: self.id,
                text,
            });
        }
        LRESULT(0)
    }

    fn on_unichar(&self, wparam: WPARAM) -> LRESULT {
        const UNICODE_NOCHAR: usize = 0xFFFF;
        if wparam.0 == UNICODE_NOCHAR {
            // Advertise UTF-32 support to senders probing for it.
            return LRESULT(1);
        }
        let cp = wparam.0 as u32;
        if (cp < 0x20 && cp != 0x09 && cp != 0x0D) || cp == 0x7F {
            return LRESULT(0);
        }
        let cp = if cp == 0x0D { 0x0A } else { cp };
        if let Some(c) = char::from_u32(cp) {
            dispatch_event(Event::TextInput {
                window: self.id,
                text: c.to_string(),
            });
        }
        LRESULT(0)
    }
}
