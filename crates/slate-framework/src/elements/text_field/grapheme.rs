//! Grapheme-aware caret motion helpers for TextField.
//!
//! All functions operate on UTF-8 byte offsets. Caret positions are always
//! at grapheme cluster boundaries — mutating via these helpers guarantees
//! `String::insert_str` / `replace_range` never panic on a mid-codepoint offset.

use unicode_segmentation::GraphemeCursor;

/// Return the byte offset of the grapheme boundary immediately before `caret`.
///
/// Clamps to 0 when already at the start of `text`.
pub(super) fn prev_grapheme_boundary(text: &str, caret: usize) -> usize {
    let mut cursor = GraphemeCursor::new(caret, text.len(), true);
    cursor.prev_boundary(text, 0).ok().flatten().unwrap_or(0)
}

/// Return the byte offset of the grapheme boundary immediately after `caret`.
///
/// Clamps to `text.len()` when already at the end.
pub(super) fn next_grapheme_boundary(text: &str, caret: usize) -> usize {
    let mut cursor = GraphemeCursor::new(caret, text.len(), true);
    cursor
        .next_boundary(text, 0)
        .ok()
        .flatten()
        .unwrap_or(text.len())
}

/// Insert `ins` at `caret` in `text` and return the new caret byte offset.
pub(super) fn insert_text_at(text: &mut String, caret: usize, ins: &str) -> usize {
    text.insert_str(caret, ins);
    caret + ins.len()
}

/// Delete the grapheme cluster immediately before `caret`.
///
/// Returns the new caret position (= start of the deleted grapheme). No-op
/// when `caret == 0`.
#[allow(dead_code)] // Used in tests; the Backspace handler inlines the pattern.
pub(super) fn delete_grapheme_before(text: &mut String, caret: usize) -> usize {
    let prev = prev_grapheme_boundary(text, caret);
    if prev < caret {
        text.replace_range(prev..caret, "");
    }
    prev
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- prev_grapheme_boundary ---

    #[test]
    fn prev_boundary_ascii() {
        assert_eq!(prev_grapheme_boundary("hello", 5), 4);
        assert_eq!(prev_grapheme_boundary("hello", 1), 0);
        assert_eq!(prev_grapheme_boundary("hello", 0), 0);
    }

    #[test]
    fn prev_boundary_cjk() {
        // "你好" — each char is 3 UTF-8 bytes
        let s = "你好";
        assert_eq!(s.len(), 6);
        assert_eq!(prev_grapheme_boundary(s, 6), 3);
        assert_eq!(prev_grapheme_boundary(s, 3), 0);
        assert_eq!(prev_grapheme_boundary(s, 0), 0);
    }

    #[test]
    fn prev_boundary_emoji() {
        // "😀" is 4 bytes; caret 4 → prev is 0
        let s = "😀";
        assert_eq!(s.len(), 4);
        assert_eq!(prev_grapheme_boundary(s, 4), 0);
        assert_eq!(prev_grapheme_boundary(s, 0), 0);
    }

    #[test]
    fn prev_boundary_combining_diacritic() {
        // "e\u{0301}" = 'e' (1 byte) + combining acute (2 bytes) = 3 bytes total
        // They form a single grapheme cluster; caret 3 → prev is 0
        let s = "e\u{0301}";
        assert_eq!(s.len(), 3);
        assert_eq!(prev_grapheme_boundary(s, 3), 0);
        assert_eq!(prev_grapheme_boundary(s, 0), 0);
    }

    // --- next_grapheme_boundary ---

    #[test]
    fn next_boundary_ascii() {
        assert_eq!(next_grapheme_boundary("hello", 0), 1);
        assert_eq!(next_grapheme_boundary("hello", 4), 5);
        assert_eq!(next_grapheme_boundary("hello", 5), 5);
    }

    #[test]
    fn next_boundary_cjk() {
        let s = "你好";
        assert_eq!(next_grapheme_boundary(s, 0), 3);
        assert_eq!(next_grapheme_boundary(s, 3), 6);
        assert_eq!(next_grapheme_boundary(s, 6), 6);
    }

    #[test]
    fn next_boundary_combining_diacritic() {
        let s = "e\u{0301}";
        // Whole cluster is one grapheme; next from 0 jumps to 3
        assert_eq!(next_grapheme_boundary(s, 0), 3);
        assert_eq!(next_grapheme_boundary(s, 3), 3);
    }

    // --- insert_text_at ---

    #[test]
    fn insert_text_at_middle() {
        let mut s = String::from("hello world");
        let new_caret = insert_text_at(&mut s, 5, ", dear");
        assert_eq!(s, "hello, dear world");
        assert_eq!(new_caret, 11);
    }

    #[test]
    fn insert_text_at_start() {
        let mut s = String::from("world");
        let new_caret = insert_text_at(&mut s, 0, "hello ");
        assert_eq!(s, "hello world");
        assert_eq!(new_caret, 6);
    }

    #[test]
    fn insert_text_at_end() {
        let mut s = String::from("hello");
        let new_caret = insert_text_at(&mut s, 5, "!");
        assert_eq!(s, "hello!");
        assert_eq!(new_caret, 6);
    }

    // --- delete_grapheme_before ---

    #[test]
    fn delete_grapheme_before_ascii() {
        let mut s = String::from("hello");
        let new_caret = delete_grapheme_before(&mut s, 5);
        assert_eq!(s, "hell");
        assert_eq!(new_caret, 4);
    }

    #[test]
    fn delete_grapheme_before_cjk() {
        let mut s = String::from("你好");
        let new_caret = delete_grapheme_before(&mut s, 6);
        assert_eq!(s, "你");
        assert_eq!(new_caret, 3);

        let new_caret2 = delete_grapheme_before(&mut s, 3);
        assert_eq!(s, "");
        assert_eq!(new_caret2, 0);
    }

    #[test]
    fn delete_grapheme_before_emoji() {
        let mut s = String::from("hi😀");
        // "hi" = 2 bytes, "😀" = 4 bytes; total 6
        let new_caret = delete_grapheme_before(&mut s, 6);
        assert_eq!(s, "hi");
        assert_eq!(new_caret, 2);
    }

    #[test]
    fn delete_grapheme_before_combining() {
        let mut s = String::from("e\u{0301}");
        let new_caret = delete_grapheme_before(&mut s, 3);
        assert_eq!(s, "");
        assert_eq!(new_caret, 0);
    }

    #[test]
    fn delete_grapheme_before_at_start() {
        let mut s = String::from("hello");
        let new_caret = delete_grapheme_before(&mut s, 0);
        assert_eq!(s, "hello");
        assert_eq!(new_caret, 0);
    }

    // --- round-trip ---

    #[test]
    fn insert_then_delete_round_trip() {
        let mut s = String::from("hello");
        let caret = insert_text_at(&mut s, 5, "😀");
        assert_eq!(caret, 9); // 5 + 4 bytes
        let caret = delete_grapheme_before(&mut s, caret);
        assert_eq!(s, "hello");
        assert_eq!(caret, 5);
    }
}
