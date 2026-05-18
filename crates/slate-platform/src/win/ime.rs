//! IMM32 FFI wrappers and helpers for Win32 IME composition handling.
//!
//! Owns the RAII `ImmContextGuard` (paired `ImmGetContext`/`ImmReleaseContext`)
//! and pure helpers for decoding composition strings, cursor positions, and
//! target-converted attribute runs. Message-pump arms in
//! [`crate::win::message_loop`] consume these helpers from the
//! `WM_IME_COMPOSITION` arm; see Phase 3 plan and ADR-001 amendment.

use std::ops::Range;

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::Input::Ime::{
    ATTR_TARGET_CONVERTED, CFS_RECT, COMPOSITIONFORM, GCS_COMPATTR, GCS_CURSORPOS, HIMC,
    IME_COMPOSITION_STRING, ImmGetCompositionStringW, ImmGetContext, ImmReleaseContext,
    ImmSetCompositionWindow,
};

/// RAII guard pairing `ImmGetContext` with `ImmReleaseContext`.
///
/// Constructed via [`ImmContextGuard::acquire`]. `Drop` releases the context
/// iff both `hwnd` and `imc` are non-default — defends against panic-path
/// unwinds and moved-from guards.
pub(crate) struct ImmContextGuard {
    imc: HIMC,
    hwnd: HWND,
}

impl ImmContextGuard {
    /// Acquire an IMM context for `hwnd`. Returns `None` if the OS reports
    /// no IMM context is associated with the window (e.g. IME disabled).
    pub(crate) fn acquire(hwnd: HWND) -> Option<Self> {
        // SAFETY: ImmGetContext is safe for any valid HWND; returns null on failure.
        let imc = unsafe { ImmGetContext(hwnd) };
        if imc.is_invalid() {
            return None;
        }
        Some(Self { imc, hwnd })
    }

    pub(crate) fn handle(&self) -> HIMC {
        self.imc
    }
}

impl Drop for ImmContextGuard {
    fn drop(&mut self) {
        if self.hwnd != HWND::default() && !self.imc.is_invalid() {
            // SAFETY: paired with the ImmGetContext call in `acquire`.
            let _ = unsafe { ImmReleaseContext(self.hwnd, self.imc) };
        }
    }
}

/// Read a composition string (UTF-16) from the IMM and convert to UTF-8.
///
/// `flag` is one of `GCS_COMPSTR` or `GCS_RESULTSTR`. Returns `None` when:
/// - the IMM call reports zero/negative byte length;
/// - the UTF-16 buffer fails to decode (fail-fast — no U+FFFD replacement).
///
/// Returns `(utf8_string, utf16_source_buffer)`. The UTF-16 buffer is
/// returned so the caller can translate UTF-16 cursor/selection offsets
/// to UTF-8 byte offsets via [`utf16_units_to_utf8_bytes`].
pub(crate) fn read_composition_string_utf8(
    imc: HIMC,
    flag: IME_COMPOSITION_STRING,
) -> Option<(String, Vec<u16>)> {
    // First call: probe required size in bytes.
    // SAFETY: passing null + 0 is the documented size-probe contract.
    let byte_len = unsafe { ImmGetCompositionStringW(imc, flag, None, 0) };
    if byte_len <= 0 {
        return None;
    }
    let byte_len = byte_len as usize;
    debug_assert!(
        byte_len.is_multiple_of(2),
        "UTF-16 byte length must be even"
    );
    let u16_len = byte_len / 2;
    let mut buf = vec![0u16; u16_len];

    // Second call: fill the buffer.
    // SAFETY: buf is valid for byte_len bytes (= u16_len * 2).
    let written = unsafe {
        ImmGetCompositionStringW(imc, flag, Some(buf.as_mut_ptr() as *mut _), byte_len as u32)
    };
    if written <= 0 {
        return None;
    }
    let written = written as usize;
    let u16_written = written / 2;
    buf.truncate(u16_written);

    match String::from_utf16(&buf) {
        Ok(s) => Some((s, buf)),
        Err(_) => {
            log::warn!(
                target: "slate::win",
                "malformed UTF-16 from IMM (flag={:?}); dropping composition",
                flag
            );
            None
        }
    }
}

/// Read the IMM cursor position (UTF-16 code unit index into the composition).
///
/// `ImmGetCompositionStringW(imc, GCS_CURSORPOS, null, 0)` returns an LRESULT
/// whose LOWORD is the cursor position and HIWORD is the anchor. We mask the
/// low 16 bits — casting the raw LRESULT to `usize` would include garbage
/// upper bits on 64-bit Windows.
///
/// Returns `None` when the IMM call fails or reports a negative value.
pub(crate) fn read_composition_cursor(imc: HIMC) -> Option<usize> {
    // SAFETY: passing null + 0 is the size-probe / no-buffer query form. For
    // GCS_CURSORPOS this is documented to return the cursor in the LRESULT.
    let ret = unsafe { ImmGetCompositionStringW(imc, GCS_CURSORPOS, None, 0) };
    if ret < 0 {
        return None;
    }
    // LRESULT is i32 here in `windows` crate; mask low 16 bits for cursor.
    Some((ret as u32 & 0xFFFF) as usize)
}

/// Read the IMM attribute byte array (one byte per UTF-16 unit).
pub(crate) fn read_composition_attrs(imc: HIMC) -> Option<Vec<u8>> {
    // SAFETY: passing null + 0 is the documented size-probe.
    let byte_len = unsafe { ImmGetCompositionStringW(imc, GCS_COMPATTR, None, 0) };
    if byte_len <= 0 {
        return None;
    }
    let byte_len = byte_len as usize;
    let mut buf = vec![0u8; byte_len];

    // SAFETY: buf is valid for byte_len bytes.
    let written = unsafe {
        ImmGetCompositionStringW(
            imc,
            GCS_COMPATTR,
            Some(buf.as_mut_ptr() as *mut _),
            byte_len as u32,
        )
    };
    if written <= 0 {
        return None;
    }
    buf.truncate(written as usize);
    Some(buf)
}

/// Translate a UTF-16 code-unit prefix length into the corresponding number
/// of UTF-8 bytes.
///
/// Iterates the UTF-16 prefix, decoding surrogate pairs, and accumulates the
/// UTF-8 byte length of each code point (1/2/3/4 bytes). Lone surrogates in
/// the prefix count as a single code unit but contribute the UTF-8 length of
/// U+FFFD (3 bytes); the actual conversion of the *whole* string is done by
/// [`String::from_utf16`] which fail-fasts on lone surrogates — so a return
/// from this helper is only consumed when the parent string converted
/// successfully.
pub(crate) fn utf16_units_to_utf8_bytes(utf16: &[u16], cursor_u16: usize) -> usize {
    let end = cursor_u16.min(utf16.len());
    let mut bytes = 0usize;
    let mut i = 0;
    while i < end {
        let unit = utf16[i];
        if (0xD800..=0xDBFF).contains(&unit) {
            // High surrogate; expect a low surrogate next within the prefix.
            if i + 1 < end {
                let low = utf16[i + 1];
                if (0xDC00..=0xDFFF).contains(&low) {
                    // Surrogate pair → U+10000..=U+10FFFF → 4 UTF-8 bytes.
                    bytes += 4;
                    i += 2;
                    continue;
                }
            }
            // Lone high surrogate inside the prefix — count as U+FFFD.
            bytes += 3;
            i += 1;
        } else if (0xDC00..=0xDFFF).contains(&unit) {
            // Orphan low — count as U+FFFD.
            bytes += 3;
            i += 1;
        } else if unit < 0x80 {
            bytes += 1;
            i += 1;
        } else if unit < 0x800 {
            bytes += 2;
            i += 1;
        } else {
            bytes += 3;
            i += 1;
        }
    }
    bytes
}

/// Find the contiguous run of `ATTR_TARGET_CONVERTED` in the IMM attribute
/// array and return its byte range in the UTF-8 composition string.
///
/// Defensively returns `None` when the attribute array length does not equal
/// the UTF-16 text length: some IMEs (per winit issue #1234) report per-code-
/// point attrs rather than per-UTF-16-unit attrs. We accept the degradation
/// (no selection highlight) rather than mis-aligning the selection range.
pub(crate) fn find_target_converted_run(attrs: &[u8], utf16_text: &[u16]) -> Option<Range<usize>> {
    if attrs.len() != utf16_text.len() {
        return None;
    }
    let target = ATTR_TARGET_CONVERTED as u8;
    let start_u16 = attrs.iter().position(|&a| a == target)?;
    let end_u16 = start_u16
        + attrs[start_u16..]
            .iter()
            .take_while(|&&a| a == target)
            .count();
    let start_byte = utf16_units_to_utf8_bytes(utf16_text, start_u16);
    let end_byte = utf16_units_to_utf8_bytes(utf16_text, end_u16);
    Some(start_byte..end_byte)
}

/// Position the IME candidate window at `caret_client_rect` (physical client
/// pixels). Uses `CFS_RECT` so the candidate window is constrained to stay
/// within the caret rect; falls back to OS default if the IME ignores the
/// hint.
pub(crate) fn set_composition_window(imc: HIMC, caret_client_rect: RECT) {
    let form = COMPOSITIONFORM {
        dwStyle: CFS_RECT,
        ptCurrentPos: POINT {
            x: caret_client_rect.left,
            y: caret_client_rect.top,
        },
        rcArea: caret_client_rect,
    };
    // SAFETY: form is a valid COMPOSITIONFORM by value; imc is owned by the
    // caller's ImmContextGuard for the duration of this call.
    let _ = unsafe { ImmSetCompositionWindow(imc, &form as *const _) };
}

// ---------------------------------------------------------------------------
// Unit tests — pure helpers only (no IMM calls).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_units_ascii() {
        let utf16: Vec<u16> = "hello".encode_utf16().collect();
        assert_eq!(utf16_units_to_utf8_bytes(&utf16, 0), 0);
        assert_eq!(utf16_units_to_utf8_bytes(&utf16, 3), 3);
        assert_eq!(utf16_units_to_utf8_bytes(&utf16, 5), 5);
        assert_eq!(utf16_units_to_utf8_bytes(&utf16, 99), 5);
    }

    #[test]
    fn utf16_units_bmp_japanese() {
        // こんにちは — each codepoint is 3 UTF-8 bytes, 1 UTF-16 unit.
        let utf16: Vec<u16> = "こんにちは".encode_utf16().collect();
        assert_eq!(utf16.len(), 5);
        assert_eq!(utf16_units_to_utf8_bytes(&utf16, 0), 0);
        assert_eq!(utf16_units_to_utf8_bytes(&utf16, 2), 6);
        assert_eq!(utf16_units_to_utf8_bytes(&utf16, 5), 15);
    }

    #[test]
    fn utf16_units_surrogate_pair_emoji() {
        // 😀 (U+1F600) — 4 UTF-8 bytes, 2 UTF-16 units (surrogate pair).
        let utf16: Vec<u16> = "a😀b".encode_utf16().collect();
        assert_eq!(utf16.len(), 4);
        assert_eq!(utf16_units_to_utf8_bytes(&utf16, 0), 0);
        assert_eq!(utf16_units_to_utf8_bytes(&utf16, 1), 1); // 'a'
        assert_eq!(utf16_units_to_utf8_bytes(&utf16, 3), 5); // 'a' + 😀
        assert_eq!(utf16_units_to_utf8_bytes(&utf16, 4), 6); // + 'b'
    }

    #[test]
    fn find_target_run_pinyin_like() {
        // 5 UTF-16 units; the middle 3 are the target-converted run.
        let utf16: Vec<u16> = "abcde".encode_utf16().collect();
        let attrs: Vec<u8> = vec![
            0,
            ATTR_TARGET_CONVERTED as u8,
            ATTR_TARGET_CONVERTED as u8,
            ATTR_TARGET_CONVERTED as u8,
            0,
        ];
        let run = find_target_converted_run(&attrs, &utf16).expect("target run present");
        assert_eq!(run, 1..4);
    }

    #[test]
    fn find_target_run_length_mismatch_returns_none() {
        // attrs reports per-codepoint (3), but text has 4 UTF-16 units (one emoji).
        let utf16: Vec<u16> = "a😀b".encode_utf16().collect();
        assert_eq!(utf16.len(), 4);
        let attrs: Vec<u8> = vec![0, ATTR_TARGET_CONVERTED as u8, 0];
        assert!(find_target_converted_run(&attrs, &utf16).is_none());
    }

    #[test]
    fn find_target_run_absent_returns_none() {
        let utf16: Vec<u16> = "abc".encode_utf16().collect();
        let attrs: Vec<u8> = vec![0, 0, 0];
        assert!(find_target_converted_run(&attrs, &utf16).is_none());
    }
}
