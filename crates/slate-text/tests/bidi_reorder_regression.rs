//! Regression gate for bidi visual-order line assembly.
//!
//! Drives the full segment → shape → fit → visual-assembly path with the
//! deterministic mock backend and locks the observable reordering: which source
//! word lands leftmost, the resolved base direction, and the level-run metadata.
//!
//! The mock shaper is direction-agnostic (one glyph per char, every glyph
//! `cluster = 0`), so the multiline path's absolute-cluster rewrite tags each
//! glyph with its *word's* start byte — that lets the asserts identify which
//! logical word a visual glyph came from without a real RTL font. Real glyph
//! shaping of complex scripts is exercised by the platform parity suites.

mod common;

use common::mock::MockBackend;
use slate_text::{Direction, TextBackend, shape_document, wrap_document};

const ADV: f32 = 10.0; // MockBackend default advance per char
const WIDE: f32 = 10_000.0; // never wrap

/// Shape `text` into a single visual line (wide box) and return it.
fn single_line(text: &str) -> slate_text::VisualLine {
    let mut backend = MockBackend::default();
    let font = backend.load_font("Test", 16.0, 1.0).unwrap();
    let doc = shape_document(&backend, &font, text).unwrap();
    let layout = wrap_document(&doc, WIDE);
    assert_eq!(layout.lines.len(), 1, "expected one visual line for {text:?}");
    layout.lines.into_iter().next().unwrap()
}

#[test]
fn rtl_base_reverses_word_order_visually() {
    // "אבג דהו": two Hebrew words, RTL base. The logically-second word must be
    // leftmost on screen; the logically-first word rightmost.
    let text = "אבג דהו"; // bytes: אבג=0..6, ' '=6..7, דהו=7..13
    let vline = single_line(text).line;

    assert_eq!(vline.base_direction, Direction::Rtl);

    // Leftmost glyph belongs to "דהו" (starts at byte 7); the absolute-cluster
    // rewrite tags every glyph of that word with cluster 7.
    let leftmost = vline.glyphs.first().unwrap();
    assert_eq!(leftmost.position_lpx[0], 0.0);
    assert_eq!(leftmost.cluster, 7, "second word is visually leftmost");

    // Rightmost glyph belongs to "אבג" (cluster 0).
    let rightmost = vline.glyphs.last().unwrap();
    assert_eq!(rightmost.cluster, 0, "first word is visually rightmost");

    // RTL runs are tagged so the caret/hit-test layer can walk them.
    assert!(!vline.runs.is_empty());
    assert!(vline.runs.iter().all(|r| r.direction == Direction::Rtl));
}

#[test]
fn ltr_base_keeps_word_order_but_tags_runs() {
    // "abc אבג": LTR base; Latin stays left, Hebrew right (no glyph reordering
    // at this granularity), but the line carries an LTR + an RTL run.
    let text = "abc אבג"; // abc=0..3, ' '=3..4, אבג=4..10
    let vline = single_line(text).line;

    assert_eq!(vline.base_direction, Direction::Ltr);

    // Latin "abc" stays leftmost.
    let leftmost = vline.glyphs.first().unwrap();
    assert_eq!(leftmost.position_lpx[0], 0.0);
    assert_eq!(leftmost.cluster, 0);

    // Two runs: a leading LTR run and a trailing RTL run.
    assert_eq!(vline.runs.len(), 2);
    assert!(vline.runs.iter().any(|r| r.direction == Direction::Ltr));
    assert!(vline.runs.iter().any(|r| r.direction == Direction::Rtl));
}

#[test]
fn pure_ltr_leaves_runs_empty_and_positions_sequential() {
    // Pure LTR keeps the Phase-1 contract: runs empty (implicit single LTR run),
    // glyphs sequentially placed.
    let line = single_line("abc def").line;
    assert_eq!(line.base_direction, Direction::Ltr);
    assert!(line.runs.is_empty(), "pure-LTR line has no explicit runs");
    for (i, g) in line.glyphs.iter().enumerate() {
        assert_eq!(g.position_lpx[0], i as f32 * ADV);
    }
}
