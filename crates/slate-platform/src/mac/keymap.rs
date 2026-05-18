//! macOS virtual-key code → W3C `KeyCode` mapping.
//!
//! Constants follow Apple's `Carbon/HIToolbox/Events.h` (`kVK_*`). Names are
//! W3C `KeyboardEvent.code` to match the cross-platform [`KeyCode`] surface.

use crate::KeyCode;

/// Decode a macOS virtual key code into the layout-independent [`KeyCode`].
///
/// Returns [`KeyCode::Unidentified`] for unknown codes.
pub(crate) fn decode_keycode(vk: u16) -> KeyCode {
    match vk {
        // Letters (kVK_ANSI_*).
        0x00 => KeyCode::KeyA,
        0x0B => KeyCode::KeyB,
        0x08 => KeyCode::KeyC,
        0x02 => KeyCode::KeyD,
        0x0E => KeyCode::KeyE,
        0x03 => KeyCode::KeyF,
        0x05 => KeyCode::KeyG,
        0x04 => KeyCode::KeyH,
        0x22 => KeyCode::KeyI,
        0x26 => KeyCode::KeyJ,
        0x28 => KeyCode::KeyK,
        0x25 => KeyCode::KeyL,
        0x2E => KeyCode::KeyM,
        0x2D => KeyCode::KeyN,
        0x1F => KeyCode::KeyO,
        0x23 => KeyCode::KeyP,
        0x0C => KeyCode::KeyQ,
        0x0F => KeyCode::KeyR,
        0x01 => KeyCode::KeyS,
        0x11 => KeyCode::KeyT,
        0x20 => KeyCode::KeyU,
        0x09 => KeyCode::KeyV,
        0x0D => KeyCode::KeyW,
        0x07 => KeyCode::KeyX,
        0x10 => KeyCode::KeyY,
        0x06 => KeyCode::KeyZ,
        // Digits (kVK_ANSI_*).
        0x1D => KeyCode::Digit0,
        0x12 => KeyCode::Digit1,
        0x13 => KeyCode::Digit2,
        0x14 => KeyCode::Digit3,
        0x15 => KeyCode::Digit4,
        0x17 => KeyCode::Digit5,
        0x16 => KeyCode::Digit6,
        0x1A => KeyCode::Digit7,
        0x1C => KeyCode::Digit8,
        0x19 => KeyCode::Digit9,
        // Function keys.
        0x7A => KeyCode::F1,
        0x78 => KeyCode::F2,
        0x63 => KeyCode::F3,
        0x76 => KeyCode::F4,
        0x60 => KeyCode::F5,
        0x61 => KeyCode::F6,
        0x62 => KeyCode::F7,
        0x64 => KeyCode::F8,
        0x65 => KeyCode::F9,
        0x6D => KeyCode::F10,
        0x67 => KeyCode::F11,
        0x6F => KeyCode::F12,
        // Navigation / whitespace.
        0x24 => KeyCode::Enter,     // kVK_Return
        0x4C => KeyCode::Enter,     // kVK_ANSI_KeypadEnter (numpad)
        0x30 => KeyCode::Tab,       // kVK_Tab
        0x33 => KeyCode::Backspace, // kVK_Delete (Mac "Delete" = Backspace)
        0x75 => KeyCode::Delete,    // kVK_ForwardDelete
        0x35 => KeyCode::Escape,    // kVK_Escape
        0x31 => KeyCode::Space,     // kVK_Space
        0x7E => KeyCode::ArrowUp,
        0x7D => KeyCode::ArrowDown,
        0x7B => KeyCode::ArrowLeft,
        0x7C => KeyCode::ArrowRight,
        0x73 => KeyCode::Home,
        0x77 => KeyCode::End,
        0x74 => KeyCode::PageUp,
        0x79 => KeyCode::PageDown,
        // Modifiers.
        0x38 => KeyCode::ShiftLeft,
        0x3C => KeyCode::ShiftRight,
        0x3B => KeyCode::ControlLeft,
        0x3E => KeyCode::ControlRight,
        0x3A => KeyCode::AltLeft, // kVK_Option
        0x3D => KeyCode::AltRight,
        0x37 => KeyCode::MetaLeft, // kVK_Command
        0x36 => KeyCode::MetaRight,
        // Punctuation (US-ANSI positional).
        0x32 => KeyCode::Backquote,
        0x1B => KeyCode::Minus,
        0x18 => KeyCode::Equal,
        0x21 => KeyCode::BracketLeft,
        0x1E => KeyCode::BracketRight,
        0x2A => KeyCode::Backslash,
        0x29 => KeyCode::Semicolon,
        0x27 => KeyCode::Quote,
        0x2B => KeyCode::Comma,
        0x2F => KeyCode::Period,
        0x2C => KeyCode::Slash,
        _ => KeyCode::Unidentified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_letters() {
        assert_eq!(decode_keycode(0x00), KeyCode::KeyA);
        assert_eq!(decode_keycode(0x0D), KeyCode::KeyW);
        assert_eq!(decode_keycode(0x06), KeyCode::KeyZ);
    }

    #[test]
    fn roundtrip_navigation() {
        assert_eq!(decode_keycode(0x24), KeyCode::Enter);
        assert_eq!(decode_keycode(0x30), KeyCode::Tab);
        assert_eq!(decode_keycode(0x35), KeyCode::Escape);
        assert_eq!(decode_keycode(0x33), KeyCode::Backspace);
        assert_eq!(decode_keycode(0x75), KeyCode::Delete);
        assert_eq!(decode_keycode(0x31), KeyCode::Space);
        assert_eq!(decode_keycode(0x7E), KeyCode::ArrowUp);
        assert_eq!(decode_keycode(0x7D), KeyCode::ArrowDown);
        assert_eq!(decode_keycode(0x7B), KeyCode::ArrowLeft);
        assert_eq!(decode_keycode(0x7C), KeyCode::ArrowRight);
        assert_eq!(decode_keycode(0x73), KeyCode::Home);
        assert_eq!(decode_keycode(0x77), KeyCode::End);
        assert_eq!(decode_keycode(0x74), KeyCode::PageUp);
        assert_eq!(decode_keycode(0x79), KeyCode::PageDown);
    }

    #[test]
    fn roundtrip_function_keys() {
        assert_eq!(decode_keycode(0x7A), KeyCode::F1);
        assert_eq!(decode_keycode(0x78), KeyCode::F2);
        assert_eq!(decode_keycode(0x6F), KeyCode::F12);
    }

    #[test]
    fn roundtrip_modifiers() {
        assert_eq!(decode_keycode(0x38), KeyCode::ShiftLeft);
        assert_eq!(decode_keycode(0x3C), KeyCode::ShiftRight);
        assert_eq!(decode_keycode(0x37), KeyCode::MetaLeft);
    }

    #[test]
    fn unknown_vk_returns_unidentified() {
        assert_eq!(decode_keycode(0xFFFF), KeyCode::Unidentified);
        assert_eq!(decode_keycode(0xFE), KeyCode::Unidentified);
    }
}
