//! UTF-16 ↔ UTF-8-byte offset conversion for the macOS `NSTextInputClient`
//! bridge.
//!
//! AppKit's `NSTextInputClient` protocol speaks **UTF-16 code-unit** offsets
//! (every `NSRange` location/length is a UTF-16 index), while the framework
//! stores **UTF-8 byte** offsets. Passing indices through untranslated lands a
//! caret on the wrong byte inside a CJK / Vietnamese composition (and corrupts
//! the reply / reconversion ranges). These two pure helpers are the single
//! source of truth for the conversion, applied at every `NSRange` boundary in
//! [`super::view_ime`].
//!
//! Kept macOS-only on purpose: Windows reads whole composition strings as
//! UTF-8 and never crosses this boundary, so generalizing into shared code
//! would add a contract Windows doesn't need.
//!
//! Both directions walk `char_indices()` accumulating per-`char` UTF-16 /
//! UTF-8 lengths, so combining marks (which are separate `char`s) are counted
//! correctly. Out-of-range inputs clamp to the string's end rather than
//! panicking — a caret past the end is reported at the end, never on a
//! non-`char`-boundary byte.

/// UTF-16 code-unit offset → UTF-8 byte offset within `s`.
///
/// Clamps a past-the-end `utf16_off` to `s.len()`. An offset that falls in the
/// middle of a surrogate pair (mid-`char`) snaps forward to the next `char`
/// boundary — a defensive case a well-behaved IME never produces.
pub(super) fn utf16_to_byte(s: &str, utf16_off: usize) -> usize {
    let mut units = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        if units >= utf16_off {
            return byte_idx;
        }
        units += ch.len_utf16();
    }
    // Reached the end: either utf16_off lands exactly at the end, or it
    // overran — both clamp to the full byte length.
    s.len()
}

/// UTF-8 byte offset → UTF-16 code-unit offset within `s`.
///
/// Clamps a past-the-end `byte_off` to the string's full UTF-16 length. A
/// `byte_off` that falls inside a multi-byte `char` counts up to (not into)
/// that `char`.
pub(super) fn byte_to_utf16(s: &str, byte_off: usize) -> usize {
    let mut units = 0usize;
    for (byte_idx, ch) in s.char_indices() {
        if byte_idx >= byte_off {
            return units;
        }
        units += ch.len_utf16();
    }
    // byte_off is at or past the end → full UTF-16 length.
    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_identity_both_directions() {
        let s = "hello";
        for i in 0..=5 {
            assert_eq!(utf16_to_byte(s, i), i, "utf16_to_byte ascii @ {i}");
            assert_eq!(byte_to_utf16(s, i), i, "byte_to_utf16 ascii @ {i}");
        }
    }

    #[test]
    fn bmp_cjk_one_unit_three_bytes() {
        // "あ" = 1 UTF-16 unit, 3 UTF-8 bytes. "あい" = 2 units, 6 bytes.
        let s = "あい";
        assert_eq!(utf16_to_byte(s, 0), 0);
        assert_eq!(utf16_to_byte(s, 1), 3);
        assert_eq!(utf16_to_byte(s, 2), 6);
        assert_eq!(byte_to_utf16(s, 0), 0);
        assert_eq!(byte_to_utf16(s, 3), 1);
        assert_eq!(byte_to_utf16(s, 6), 2);
    }

    #[test]
    fn astral_emoji_is_surrogate_pair() {
        // "😀" = U+1F600 = 2 UTF-16 units (surrogate pair), 4 UTF-8 bytes.
        let s = "a😀b";
        assert_eq!(utf16_to_byte(s, 0), 0); // 'a'
        assert_eq!(utf16_to_byte(s, 1), 1); // start of emoji
        assert_eq!(utf16_to_byte(s, 3), 5); // 'b' (1 + 2 surrogate units)
        assert_eq!(utf16_to_byte(s, 4), 6); // end
        assert_eq!(byte_to_utf16(s, 1), 1);
        assert_eq!(byte_to_utf16(s, 5), 3);
        assert_eq!(byte_to_utf16(s, 6), 4);
    }

    #[test]
    fn vietnamese_precomposed_vs_decomposed() {
        // Precomposed "ế" U+1EBF = 1 char, 1 UTF-16 unit, 3 bytes.
        let pre = "tiế";
        assert_eq!(pre.chars().count(), 3);
        assert_eq!(utf16_to_byte(pre, 3), pre.len());
        assert_eq!(byte_to_utf16(pre, pre.len()), 3);
        // Decomposed "ế" = 'e' + U+0302 + U+0301 = 3 chars; combining marks
        // are separate chars, each 1 UTF-16 unit / 2 bytes.
        let dec = "e\u{0302}\u{0301}";
        assert_eq!(dec.chars().count(), 3);
        assert_eq!(utf16_to_byte(dec, 0), 0);
        assert_eq!(utf16_to_byte(dec, 1), 1); // after 'e'
        assert_eq!(utf16_to_byte(dec, 2), 3); // after first combining mark
        assert_eq!(utf16_to_byte(dec, 3), 5); // after second
        assert_eq!(byte_to_utf16(dec, 5), 3);
    }

    #[test]
    fn utf16_to_byte_mid_surrogate_snaps_forward() {
        // "a😀b": 'a'=1 unit, '😀'=2 units (surrogate pair), 'b'=1 unit.
        // UTF-16 offset 2 lands in the MIDDLE of the surrogate pair — a
        // malformed input a well-behaved IME never sends. The helper snaps
        // forward to the next char boundary (byte 5, start of 'b').
        let s = "a😀b";
        assert_eq!(utf16_to_byte(s, 2), 5);
    }

    #[test]
    fn offset_at_len_round_trips() {
        let s = "あ";
        let u = byte_to_utf16(s, s.len());
        assert_eq!(u, 1);
        assert_eq!(utf16_to_byte(s, u), s.len());
    }

    #[test]
    fn offset_past_end_clamps() {
        let s = "あい"; // 2 units, 6 bytes
        assert_eq!(utf16_to_byte(s, 99), 6);
        assert_eq!(byte_to_utf16(s, 99), 2);
    }

    #[test]
    fn empty_string_is_zero() {
        assert_eq!(utf16_to_byte("", 0), 0);
        assert_eq!(utf16_to_byte("", 5), 0);
        assert_eq!(byte_to_utf16("", 0), 0);
        assert_eq!(byte_to_utf16("", 5), 0);
    }
}
