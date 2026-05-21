//! Regression gate for the paragraph-shaping segmentation seam.
//!
//! `shape_words` drives off the `LineSegmenter`/`WhitespaceSegmenter` seam
//! instead of an inline byte scan. This locks the seam's observable output so
//! later work (bidi segmenter, UAX #14 fit) cannot silently drift the
//! segmentation that the editing caret math depends on.
//!
//! The gate asserts the **structural** invariants that segment-boundary drift
//! would break — item count, `is_space_run`, absolute `source_byte_range`,
//! per-item glyph ids/positions, and the new `direction` tag (always `Ltr` on
//! the whitespace path). Real complex-script cluster correctness on the native
//! shaper is covered by the platform `coretext_shaping` / parity suites.

mod common;

use common::mock::MockBackend;
use slate_text::{Direction, TextBackend, shape_words};

const ADV: f32 = 10.0; // MockBackend default advance per char
const EPS: f32 = 1e-4;

/// Shape `text` with the deterministic mock backend and return the items.
fn words(text: &str) -> Vec<slate_text::ShapedWord> {
    let backend = MockBackend::default();
    let mut font_backend = MockBackend::default();
    let font = font_backend.load_font("Test", 16.0, 1.0).unwrap();
    let (items, _space_w) = shape_words(&backend, &font, text).unwrap();
    items
}

/// Every glyph emitted by the whitespace path is tagged `Ltr`.
fn assert_all_ltr(items: &[slate_text::ShapedWord]) {
    for it in items {
        for g in &it.glyphs {
            assert_eq!(g.direction, Direction::Ltr, "whitespace path is LTR-only");
        }
    }
}

#[test]
fn empty_yields_no_items() {
    assert!(words("").is_empty());
}

#[test]
fn whitespace_only_yields_single_space_run() {
    let items = words("   ");
    assert_eq!(items.len(), 1);
    assert!(items[0].is_space_run);
    assert_eq!(items[0].source_byte_range, 0..3);
    assert_eq!(items[0].glyphs.len(), 3, "one glyph per space byte");
    assert_all_ltr(&items);
}

#[test]
fn words_and_spaces_interleave_in_source_order() {
    // "ab cd" → [word "ab", space " ", word "cd"]
    let items = words("ab cd");
    assert_eq!(items.len(), 3);

    assert!(!items[0].is_space_run);
    assert_eq!(items[0].source_byte_range, 0..2);
    assert_eq!(items[0].glyphs.len(), 2);

    assert!(items[1].is_space_run);
    assert_eq!(items[1].source_byte_range, 2..3);
    assert_eq!(items[1].glyphs.len(), 1);

    assert!(!items[2].is_space_run);
    assert_eq!(items[2].source_byte_range, 3..5);
    assert_eq!(items[2].glyphs.len(), 2);

    // Positions are word-origin-relative (pen restarts per item).
    assert!((items[2].glyphs[0].position_lpx[0] - 0.0).abs() < EPS);
    assert!((items[2].glyphs[1].position_lpx[0] - ADV).abs() < EPS);
    // glyph_id is segment-local (MockBackend starts at 1 per shape_line call).
    assert_eq!(items[2].glyphs[0].glyph_id, 1);
    assert_eq!(items[2].glyphs[1].glyph_id, 2);

    assert_all_ltr(&items);
}

#[test]
fn multiple_spaces_form_one_run() {
    // "a  b" → [word "a", space "  " (2 glyphs), word "b"]
    let items = words("a  b");
    assert_eq!(items.len(), 3);
    assert!(items[1].is_space_run);
    assert_eq!(items[1].source_byte_range, 1..3);
    assert_eq!(items[1].glyphs.len(), 2);
    assert_all_ltr(&items);
}

#[test]
fn leading_and_trailing_spaces_preserved() {
    // " ab " → [space " ", word "ab", space " "]
    let items = words(" ab ");
    assert_eq!(items.len(), 3);
    assert!(items[0].is_space_run);
    assert_eq!(items[0].source_byte_range, 0..1);
    assert!(!items[1].is_space_run);
    assert_eq!(items[1].source_byte_range, 1..3);
    assert!(items[2].is_space_run);
    assert_eq!(items[2].source_byte_range, 3..4);
    assert_all_ltr(&items);
}

#[test]
fn multibyte_word_keeps_byte_ranges_absolute() {
    // "é x": "é" is 2 bytes (U+00E9), so the space starts at byte 2.
    let items = words("é x");
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].source_byte_range, 0..2);
    assert!(items[1].is_space_run);
    assert_eq!(items[1].source_byte_range, 2..3);
    assert_eq!(items[2].source_byte_range, 3..4);
    assert_all_ltr(&items);
}
