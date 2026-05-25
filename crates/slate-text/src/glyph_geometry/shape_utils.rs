//! Grapheme-snap helpers shared by the caret / hit-test paths.

use unicode_segmentation::UnicodeSegmentation;

/// Largest grapheme-cluster boundary `<= byte` in `text`. Clamped to
/// `[0, text.len()]`.
pub(super) fn snap_grapheme_floor(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    let mut last = 0usize;
    for (b, _) in text.grapheme_indices(true) {
        if b > byte {
            return last;
        }
        last = b;
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_floor_basics() {
        assert_eq!(snap_grapheme_floor("abc", 0), 0);
        assert_eq!(snap_grapheme_floor("abc", 1), 1);
        assert_eq!(snap_grapheme_floor("abc", 3), 3);
        assert_eq!(snap_grapheme_floor("abc", 999), 3);
        // "é" NFD: graphemes start at 0 only; byte 1, 2 snap to 0.
        let s = "e\u{0301}";
        assert_eq!(snap_grapheme_floor(s, 1), 0);
        assert_eq!(snap_grapheme_floor(s, 2), 0);
        assert_eq!(snap_grapheme_floor(s, 3), 3);
    }
}
