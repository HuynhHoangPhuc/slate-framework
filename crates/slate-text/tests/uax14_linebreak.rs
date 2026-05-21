//! Integration gate for UAX #14 line-breaking + tab stops over the full
//! shape → fit → assemble path with the deterministic mock backend.
//!
//! The mock advances every char (incl. space) by `ADV` lpx, so widths are exact
//! arithmetic. It locks the behaviour the unit tests cannot: that the break
//! table actually drives `wrap_document`'s line splits (CJK wraps, ASCII still
//! wraps at spaces, RTL wraps and stays RTL) and that a tab snaps the pen to the
//! next stop in the assembled line.

mod common;

use common::mock::MockBackend;
use slate_text::{Direction, TextBackend, shape_document, wrap_document};

const ADV: f32 = 10.0; // MockBackend advance per char (space included)
const TAB_WIDTH: f32 = 4.0 * ADV; // 4 × space advance = 40 lpx
const WIDE: f32 = 10_000.0; // never wrap

fn layout(text: &str, max_width: f32) -> slate_text::MultilineLayout {
    let mut backend = MockBackend::default();
    let font = backend.load_font("Test", 16.0, 1.0).unwrap();
    let doc = shape_document(&backend, &font, text).unwrap();
    wrap_document(&doc, max_width)
}

/// CJK has no spaces but must still wrap: UAX #14 allows a break between every
/// ideograph. "日本語日本語" = 6 ideographs (3 bytes each); at width 35 only
/// three (30 lpx) fit per line.
#[test]
fn cjk_wraps_between_ideographs() {
    let text = "日本語日本語";
    let l = layout(text, 35.0);
    assert_eq!(l.lines.len(), 2, "six ideographs at width 35 → two lines");
    assert_eq!(l.lines[0].line.width_lpx, 30.0);
    // Byte coverage is contiguous and total.
    assert_eq!(l.lines[0].byte_start, 0);
    assert_eq!(l.lines[0].byte_end, 9); // first three ideographs
    assert_eq!(l.lines[1].byte_start, 9);
    assert_eq!(l.lines[1].byte_end, text.len());
}

/// A single ideograph wider than the box still gets its own line rather than
/// looping forever (no interior break inside one char).
#[test]
fn single_ideograph_wider_than_box_is_one_line() {
    let l = layout("日", 5.0);
    assert_eq!(l.lines.len(), 1);
    assert_eq!(l.lines[0].byte_end, "日".len());
}

/// Regression: ASCII still breaks only at spaces (UAX #14 break points coincide
/// with the space boundaries the old split used), never mid-word.
#[test]
fn ascii_still_wraps_at_spaces() {
    // "aaa bbb": each word 30 lpx, space 10. Width 35 fits one word per line.
    let l = layout("aaa bbb", 35.0);
    assert_eq!(l.lines.len(), 2);
    assert_eq!(l.lines[0].byte_start, 0);
    assert_eq!(l.lines[0].byte_end, 4); // "aaa " incl joining space
    assert_eq!(l.lines[1].byte_start, 4);
    assert_eq!(l.lines[1].byte_end, 7);
}

/// A tab snaps the pen to the next tab stop (multiple of `TAB_WIDTH`), measured
/// from the line origin — not the shaper's raw tab advance.
#[test]
fn tab_advances_to_next_stop() {
    // "aa\tb": pen after "aa" = 20 → next stop at 40 (tab advance 20), then 'b'
    // → line width 50. Naive (per-char) advance would be 40.
    let l = layout("aa\tb", WIDE);
    assert_eq!(l.lines.len(), 1);
    assert_eq!(l.lines[0].line.width_lpx, 50.0);

    // The tab is caret-addressable: its glyph sits at the tab's source byte (2)
    // and carries the whole stop advance (20).
    let tab_glyph = l.lines[0]
        .line
        .glyphs
        .iter()
        .find(|g| g.cluster == 2)
        .expect("tab glyph keyed to its source byte");
    assert_eq!(tab_glyph.x_advance_lpx, 20.0);
    assert_eq!(tab_glyph.position_lpx[0], 20.0);
}

/// A tab already sitting on a stop advances to the *next* stop (never collapses
/// to zero width).
#[test]
fn tab_on_exact_stop_advances_full_width() {
    // "aaaa\tb": pen after "aaaa" = 40 (exactly one stop) → next stop 80, tab
    // advance 40, then 'b' → width 90.
    let l = layout("aaaa\tb", WIDE);
    assert_eq!(l.lines[0].line.width_lpx, 40.0 + TAB_WIDTH + ADV);
}

/// An RTL paragraph wider than the box wraps like LTR (same width arithmetic),
/// and every wrapped visual line keeps the RTL base direction so line assembly
/// reorders each one.
#[test]
fn rtl_paragraph_wraps_and_each_line_stays_rtl() {
    // Three Hebrew words (3 chars = 30 lpx each) joined by spaces; width 45 fits
    // one word + the absorbed space per line → three lines.
    let text = "אבג דהו אבג";
    let l = layout(text, 45.0);
    assert!(l.lines.len() >= 2, "RTL paragraph must wrap");
    for (i, vline) in l.lines.iter().enumerate() {
        assert_eq!(
            vline.line.base_direction,
            Direction::Rtl,
            "wrapped line {i} should keep the RTL base direction"
        );
    }
}

// ── Mandatory breaks + CRLF normalization (document layer) ────────────────────
//
// These assert that the new `split_paragraphs` drives `shape_document`'s line
// splits. The shared mock keys every glyph to cluster 0, so caret-byte precision
// (e.g. `line_caret_end == 2`) is verified in `multiline_layout.rs` (its mock
// sets `cluster = byte offset`); here we assert line structure + byte coverage.

/// CRLF hard-breaks the document into two lines; the `\r\n` (2 bytes) folds
/// into the preceding line's coverage so byte ranges stay gap-free and total.
#[test]
fn crlf_hard_breaks_and_coverage_is_total() {
    let text = "ab\r\ncd";
    let l = layout(text, WIDE);
    assert_eq!(l.lines.len(), 2, "CRLF must hard-break into two lines");
    assert_eq!(l.lines[0].byte_start, 0);
    assert_eq!(l.lines[0].byte_end, 4); // "ab" + folded "\r\n"
    assert_eq!(l.lines[1].byte_start, 4);
    assert_eq!(l.lines[1].byte_end, text.len());
}

/// U+2028 LINE SEPARATOR is a UAX #14 mandatory break: it hard-breaks the
/// document into two lines (was previously demoted to a soft opportunity).
#[test]
fn unicode_line_separator_hard_breaks() {
    let text = "ab\u{2028}cd";
    let l = layout(text, WIDE);
    assert_eq!(l.lines.len(), 2, "U+2028 must hard-break into two lines");
    assert_eq!(l.lines[1].byte_start, "ab\u{2028}".len());
    assert_eq!(l.lines[1].byte_end, text.len());
}

/// Regression: `\n`-only layout is byte-identical to pre-change behaviour —
/// "ab\ncd" yields line ranges 0..3 and 3..5.
#[test]
fn lf_only_layout_unchanged() {
    let l = layout("ab\ncd", WIDE);
    assert_eq!(l.lines.len(), 2);
    assert_eq!((l.lines[0].byte_start, l.lines[0].byte_end), (0, 3));
    assert_eq!((l.lines[1].byte_start, l.lines[1].byte_end), (3, 5));
}
