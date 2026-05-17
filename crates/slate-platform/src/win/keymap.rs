//! Win32 virtual-key + scancode → W3C `KeyCode` mapping.
//!
//! Win32's `VK_*` codes collapse left/right modifier pairs onto a single value;
//! we use the `lparam` scancode (bits 16..23) plus the "extended" bit (bit 24)
//! to recover positional information. Names follow W3C `KeyboardEvent.code` to
//! match the cross-platform [`KeyCode`] surface.

use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_BACK, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F2, VK_F3,
    VK_F4, VK_F5, VK_F6, VK_F7, VK_F8, VK_F9, VK_F10, VK_F11, VK_F12, VK_HOME, VK_LEFT, VK_LWIN,
    VK_MENU, VK_NEXT, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};

use crate::{KeyCode, Modifiers, NamedKey};

/// Decode a Win32 virtual-key + scancode into the layout-independent [`KeyCode`].
///
/// `scancode` disambiguates `ShiftLeft` (`0x2A`) vs `ShiftRight` (`0x36`).
/// `extended` (lparam bit 24) disambiguates `ControlRight`/`AltRight` and
/// numpad vs navigation block. Numpad `Enter` collapses to `KeyCode::Enter`
/// (matches macOS behavior).
pub(crate) fn decode_keycode(vk: u32, scancode: u32, extended: bool) -> KeyCode {
    match vk {
        // Letters — VK_A..VK_Z is contiguous 0x41..0x5A == ASCII 'A'..'Z'.
        0x41..=0x5A => letter_code(vk),
        // Digits — VK_0..VK_9 is contiguous 0x30..0x39 == ASCII '0'..'9'.
        0x30..=0x39 => digit_code(vk),
        // Modifiers.
        v if v == VK_SHIFT.0 as u32 => {
            if scancode == 0x36 {
                KeyCode::ShiftRight
            } else {
                KeyCode::ShiftLeft
            }
        }
        v if v == VK_CONTROL.0 as u32 => {
            if extended {
                KeyCode::ControlRight
            } else {
                KeyCode::ControlLeft
            }
        }
        v if v == VK_MENU.0 as u32 => {
            if extended {
                KeyCode::AltRight
            } else {
                KeyCode::AltLeft
            }
        }
        v if v == VK_LWIN.0 as u32 => KeyCode::MetaLeft,
        v if v == VK_RWIN.0 as u32 => KeyCode::MetaRight,
        // Whitespace / navigation.
        v if v == VK_RETURN.0 as u32 => KeyCode::Enter,
        v if v == VK_TAB.0 as u32 => KeyCode::Tab,
        v if v == VK_ESCAPE.0 as u32 => KeyCode::Escape,
        v if v == VK_BACK.0 as u32 => KeyCode::Backspace,
        v if v == VK_DELETE.0 as u32 => KeyCode::Delete,
        v if v == VK_SPACE.0 as u32 => KeyCode::Space,
        v if v == VK_LEFT.0 as u32 => KeyCode::ArrowLeft,
        v if v == VK_RIGHT.0 as u32 => KeyCode::ArrowRight,
        v if v == VK_UP.0 as u32 => KeyCode::ArrowUp,
        v if v == VK_DOWN.0 as u32 => KeyCode::ArrowDown,
        v if v == VK_HOME.0 as u32 => KeyCode::Home,
        v if v == VK_END.0 as u32 => KeyCode::End,
        v if v == VK_PRIOR.0 as u32 => KeyCode::PageUp,
        v if v == VK_NEXT.0 as u32 => KeyCode::PageDown,
        // Function keys (VK_F1..VK_F12 are contiguous).
        v if v == VK_F1.0 as u32 => KeyCode::F1,
        v if v == VK_F2.0 as u32 => KeyCode::F2,
        v if v == VK_F3.0 as u32 => KeyCode::F3,
        v if v == VK_F4.0 as u32 => KeyCode::F4,
        v if v == VK_F5.0 as u32 => KeyCode::F5,
        v if v == VK_F6.0 as u32 => KeyCode::F6,
        v if v == VK_F7.0 as u32 => KeyCode::F7,
        v if v == VK_F8.0 as u32 => KeyCode::F8,
        v if v == VK_F9.0 as u32 => KeyCode::F9,
        v if v == VK_F10.0 as u32 => KeyCode::F10,
        v if v == VK_F11.0 as u32 => KeyCode::F11,
        v if v == VK_F12.0 as u32 => KeyCode::F12,
        _ => KeyCode::Unidentified,
    }
}

/// Return a [`NamedKey`] for any non-character key Win32 fires. Returns `None`
/// for printable keys (letters, digits, punctuation) — those carry their text
/// payload through `WM_CHAR` / [`Event::TextInput`].
pub(crate) fn vk_to_named_key(vk: u32) -> Option<NamedKey> {
    let nk = match vk {
        v if v == VK_RETURN.0 as u32 => NamedKey::Enter,
        v if v == VK_TAB.0 as u32 => NamedKey::Tab,
        v if v == VK_ESCAPE.0 as u32 => NamedKey::Escape,
        v if v == VK_BACK.0 as u32 => NamedKey::Backspace,
        v if v == VK_DELETE.0 as u32 => NamedKey::Delete,
        v if v == VK_SPACE.0 as u32 => NamedKey::Space,
        v if v == VK_LEFT.0 as u32 => NamedKey::ArrowLeft,
        v if v == VK_RIGHT.0 as u32 => NamedKey::ArrowRight,
        v if v == VK_UP.0 as u32 => NamedKey::ArrowUp,
        v if v == VK_DOWN.0 as u32 => NamedKey::ArrowDown,
        v if v == VK_HOME.0 as u32 => NamedKey::Home,
        v if v == VK_END.0 as u32 => NamedKey::End,
        v if v == VK_PRIOR.0 as u32 => NamedKey::PageUp,
        v if v == VK_NEXT.0 as u32 => NamedKey::PageDown,
        v if v == VK_F1.0 as u32 => NamedKey::F1,
        v if v == VK_F2.0 as u32 => NamedKey::F2,
        v if v == VK_F3.0 as u32 => NamedKey::F3,
        v if v == VK_F4.0 as u32 => NamedKey::F4,
        v if v == VK_F5.0 as u32 => NamedKey::F5,
        v if v == VK_F6.0 as u32 => NamedKey::F6,
        v if v == VK_F7.0 as u32 => NamedKey::F7,
        v if v == VK_F8.0 as u32 => NamedKey::F8,
        v if v == VK_F9.0 as u32 => NamedKey::F9,
        v if v == VK_F10.0 as u32 => NamedKey::F10,
        v if v == VK_F11.0 as u32 => NamedKey::F11,
        v if v == VK_F12.0 as u32 => NamedKey::F12,
        v if v == VK_SHIFT.0 as u32 => NamedKey::Shift,
        v if v == VK_CONTROL.0 as u32 => NamedKey::Control,
        v if v == VK_MENU.0 as u32 => NamedKey::Alt,
        v if v == VK_LWIN.0 as u32 || v == VK_RWIN.0 as u32 => NamedKey::Meta,
        _ => return None,
    };
    Some(nk)
}

/// Snapshot modifier state from `GetKeyState`. Used as the single source of
/// truth for keyboard event modifier reads (mouse paths keep their own
/// wparam-based reader for consistency with the existing API).
pub(crate) fn read_modifiers() -> Modifiers {
    // SAFETY: GetKeyState is documented thread-safe and reads in-process
    // keyboard state; no aliasing concerns.
    unsafe {
        Modifiers {
            shift: GetKeyState(VK_SHIFT.0 as i32) < 0,
            ctrl: GetKeyState(VK_CONTROL.0 as i32) < 0,
            alt: GetKeyState(VK_MENU.0 as i32) < 0,
            meta: GetKeyState(VK_LWIN.0 as i32) < 0 || GetKeyState(VK_RWIN.0 as i32) < 0,
        }
    }
}

fn letter_code(vk: u32) -> KeyCode {
    match vk {
        0x41 => KeyCode::KeyA,
        0x42 => KeyCode::KeyB,
        0x43 => KeyCode::KeyC,
        0x44 => KeyCode::KeyD,
        0x45 => KeyCode::KeyE,
        0x46 => KeyCode::KeyF,
        0x47 => KeyCode::KeyG,
        0x48 => KeyCode::KeyH,
        0x49 => KeyCode::KeyI,
        0x4A => KeyCode::KeyJ,
        0x4B => KeyCode::KeyK,
        0x4C => KeyCode::KeyL,
        0x4D => KeyCode::KeyM,
        0x4E => KeyCode::KeyN,
        0x4F => KeyCode::KeyO,
        0x50 => KeyCode::KeyP,
        0x51 => KeyCode::KeyQ,
        0x52 => KeyCode::KeyR,
        0x53 => KeyCode::KeyS,
        0x54 => KeyCode::KeyT,
        0x55 => KeyCode::KeyU,
        0x56 => KeyCode::KeyV,
        0x57 => KeyCode::KeyW,
        0x58 => KeyCode::KeyX,
        0x59 => KeyCode::KeyY,
        0x5A => KeyCode::KeyZ,
        _ => KeyCode::Unidentified,
    }
}

fn digit_code(vk: u32) -> KeyCode {
    match vk {
        0x30 => KeyCode::Digit0,
        0x31 => KeyCode::Digit1,
        0x32 => KeyCode::Digit2,
        0x33 => KeyCode::Digit3,
        0x34 => KeyCode::Digit4,
        0x35 => KeyCode::Digit5,
        0x36 => KeyCode::Digit6,
        0x37 => KeyCode::Digit7,
        0x38 => KeyCode::Digit8,
        0x39 => KeyCode::Digit9,
        _ => KeyCode::Unidentified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_scancode_disambiguates() {
        assert_eq!(
            decode_keycode(VK_SHIFT.0 as u32, 0x2A, false),
            KeyCode::ShiftLeft
        );
        assert_eq!(
            decode_keycode(VK_SHIFT.0 as u32, 0x36, false),
            KeyCode::ShiftRight
        );
    }

    #[test]
    fn ctrl_extended_disambiguates() {
        assert_eq!(
            decode_keycode(VK_CONTROL.0 as u32, 0, false),
            KeyCode::ControlLeft
        );
        assert_eq!(
            decode_keycode(VK_CONTROL.0 as u32, 0, true),
            KeyCode::ControlRight
        );
    }

    #[test]
    fn alt_extended_disambiguates() {
        assert_eq!(decode_keycode(VK_MENU.0 as u32, 0, false), KeyCode::AltLeft);
        assert_eq!(decode_keycode(VK_MENU.0 as u32, 0, true), KeyCode::AltRight);
    }

    #[test]
    fn letters_roundtrip() {
        assert_eq!(decode_keycode(0x41, 0, false), KeyCode::KeyA);
        assert_eq!(decode_keycode(0x57, 0, false), KeyCode::KeyW);
        assert_eq!(decode_keycode(0x5A, 0, false), KeyCode::KeyZ);
    }

    #[test]
    fn digits_roundtrip() {
        assert_eq!(decode_keycode(0x30, 0, false), KeyCode::Digit0);
        assert_eq!(decode_keycode(0x39, 0, false), KeyCode::Digit9);
    }

    #[test]
    fn function_keys_roundtrip() {
        assert_eq!(decode_keycode(VK_F1.0 as u32, 0, false), KeyCode::F1);
        assert_eq!(decode_keycode(VK_F12.0 as u32, 0, false), KeyCode::F12);
    }

    #[test]
    fn navigation_roundtrip() {
        assert_eq!(decode_keycode(VK_RETURN.0 as u32, 0, false), KeyCode::Enter);
        assert_eq!(decode_keycode(VK_TAB.0 as u32, 0, false), KeyCode::Tab);
        assert_eq!(
            decode_keycode(VK_ESCAPE.0 as u32, 0, false),
            KeyCode::Escape
        );
        assert_eq!(
            decode_keycode(VK_BACK.0 as u32, 0, false),
            KeyCode::Backspace
        );
        assert_eq!(
            decode_keycode(VK_DELETE.0 as u32, 0, false),
            KeyCode::Delete
        );
        assert_eq!(
            decode_keycode(VK_LEFT.0 as u32, 0, false),
            KeyCode::ArrowLeft
        );
    }

    #[test]
    fn unknown_vk_maps_unidentified() {
        assert_eq!(decode_keycode(0xFE, 0, false), KeyCode::Unidentified);
    }

    #[test]
    fn named_key_for_letter_is_none() {
        assert!(vk_to_named_key(0x41).is_none());
    }

    #[test]
    fn named_key_for_enter() {
        assert_eq!(vk_to_named_key(VK_RETURN.0 as u32), Some(NamedKey::Enter));
    }

    #[test]
    fn named_key_for_meta() {
        assert_eq!(vk_to_named_key(VK_LWIN.0 as u32), Some(NamedKey::Meta));
        assert_eq!(vk_to_named_key(VK_RWIN.0 as u32), Some(NamedKey::Meta));
    }
}
