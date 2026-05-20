//! Unicode word-boundary lookup for double-click word selection.
//!
//! Lives in the shared `text_edit` core (the single-line `TextField` can adopt
//! the same double-click behavior later). [`word_range_at`] maps a byte offset
//! to the span of the Unicode word segment that contains it, using
//! `split_word_bound_indices` (UAX #29). Whitespace runs are their own
//! segments, so clicking on whitespace selects the whole run; CJK runs and
//! emoji ZWJ sequences are grouped as the segmenter dictates.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

/// Absolute byte range of the Unicode word segment containing `byte`.
///
/// `byte` is treated as a caret position: the segment claimed is the one whose
/// half-open `[start, end)` contains it (so a byte exactly on a segment
/// boundary selects the following segment). A `byte` at or past `text.len()`
/// selects the final segment; empty text yields `byte..byte`.
pub(crate) fn word_range_at(text: &str, byte: usize) -> Range<usize> {
    let mut last = byte..byte;
    for (start, seg) in text.split_word_bound_indices() {
        let end = start + seg.len();
        if byte < end {
            return start..end;
        }
        last = start..end;
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_word_selects_that_word() {
        let t = "hello world";
        assert_eq!(word_range_at(t, 2), 0..5, "byte 2 inside 'hello'");
        assert_eq!(word_range_at(t, 7), 6..11, "byte 7 inside 'world'");
        // Byte at the very end selects the final word.
        assert_eq!(word_range_at(t, 11), 6..11);
    }

    #[test]
    fn on_whitespace_selects_the_whitespace_run() {
        // Single space.
        assert_eq!(word_range_at("hello world", 5), 5..6);
        // Multi-space run is one segment.
        let t = "a   b"; // bytes: a(0) sp(1) sp(2) sp(3) b(4)
        assert_eq!(word_range_at(t, 2), 1..4, "click in the run → whole run");
    }

    #[test]
    fn cjk_run_matches_unicode_word_rules() {
        let t = "東京タワー";
        let r = word_range_at(t, 6); // mid-byte (each CJK char is 3 bytes)
        // Whatever the segmenter groups, the result must equal one of its
        // segments (consistency with split_word_bound_indices).
        let segs: Vec<_> = t.split_word_bound_indices().map(|(s, w)| s..s + w.len()).collect();
        assert!(segs.contains(&r), "{r:?} must be a word-bound segment of {segs:?}");
        assert!(r.start <= 6 && 6 < r.end, "range must contain the click byte");
    }

    #[test]
    fn emoji_zwj_sequence_is_one_unit() {
        // Family emoji = 👨‍👩‍👧 joined by ZWJ; one grapheme/word unit.
        let family = "👨\u{200D}👩\u{200D}👧";
        let t = format!("{family}!");
        // A byte in the middle of the ZWJ sequence selects the whole sequence.
        let mid = family.len() / 2;
        let r = word_range_at(&t, mid);
        assert_eq!(r, 0..family.len(), "ZWJ sequence selected as one unit");
    }

    #[test]
    fn empty_text_is_empty_range() {
        assert_eq!(word_range_at("", 0), 0..0);
    }
}
