//! UTF-16 code-unit → UTF-8 byte offset mapping for cluster derivation.
//!
//! Platform shapers (CoreText, DirectWrite) report cluster indices as UTF-16
//! code-unit offsets into the source string. `ShapedGlyph::cluster` stores
//! UTF-8 byte offsets (HarfBuzz convention). This helper bridges the two.

/// Build a UTF-16 code-unit → UTF-8 byte-offset map for `text`.
///
/// The returned vector has length `utf16_len + 1`. For BMP characters, one
/// UTF-16 unit maps to the leading byte of the corresponding UTF-8 sequence.
/// For supplementary-plane characters (surrogate pairs), both UTF-16 units map
/// to the same leading UTF-8 byte. The trailing slot equals `text.len()` so
/// look-ups at the end-of-string position are well-defined.
pub(crate) fn utf16_to_utf8_byte_map(text: &str) -> Vec<u32> {
    let utf16_len: usize = text.chars().map(|c| c.len_utf16()).sum();
    let mut map = vec![0u32; utf16_len + 1];
    let mut utf16_idx = 0usize;
    for (utf8_byte, ch) in text.char_indices() {
        let units = ch.len_utf16();
        for k in 0..units {
            map[utf16_idx + k] = utf8_byte as u32;
        }
        utf16_idx += units;
    }
    map[utf16_len] = text.len() as u32;
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_one_to_one() {
        let m = utf16_to_utf8_byte_map("abc");
        assert_eq!(m, vec![0, 1, 2, 3]);
    }

    #[test]
    fn hiragana_three_byte_chars() {
        // こ=3 bytes, ん=3 bytes; both BMP → 1 UTF-16 unit each.
        let m = utf16_to_utf8_byte_map("こん");
        assert_eq!(m, vec![0, 3, 6]);
    }

    #[test]
    fn emoji_supplementary_pair() {
        // 🇯 is U+1F1EF (supplementary, 4 bytes UTF-8, 2 UTF-16 units).
        // a (byte 0, 1 U16) | 🇯 (byte 1, 2 U16 → both halves map to byte 1)
        // | b (byte 5, 1 U16) | end (byte 6).
        let s = "a🇯b";
        let m = utf16_to_utf8_byte_map(s);
        assert_eq!(m, vec![0, 1, 1, 5, 6]);
    }

    #[test]
    fn empty_string() {
        let m = utf16_to_utf8_byte_map("");
        assert_eq!(m, vec![0]);
    }

    #[test]
    fn zwj_sequence() {
        // "a👨‍👦b": a (1 byte, 1 U16) | 👨 U+1F468 (4 bytes, 2 U16)
        // | ZWJ U+200D (3 bytes, 1 U16) | 👦 U+1F466 (4 bytes, 2 U16) | b (1 byte, 1 U16).
        // Each codepoint is independent in the map even though Unicode treats
        // the ZWJ sequence as one grapheme cluster.
        let s = "a\u{1F468}\u{200D}\u{1F466}b";
        let m = utf16_to_utf8_byte_map(s);
        // bytes: a@0, 👨@1, ZWJ@5, 👦@8, b@12, end@13
        // U16:   a(1)+👨(2)+ZWJ(1)+👦(2)+b(1) = 7 units → len 8
        assert_eq!(m, vec![0, 1, 1, 5, 8, 8, 12, 13]);
    }
}
