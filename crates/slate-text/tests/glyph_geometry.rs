//! Golden-string round-trip tests for `pixel_x_at_byte` / `byte_at_pixel_x`.
//!
//! Each string is shaped by the live platform backend (CoreText on macOS,
//! DirectWrite on Windows) so cluster values come from the real shaping path.
//! For every grapheme boundary in the source we assert
//! `byte_at_pixel_x(pixel_x_at_byte(b)) == b`, plus the end-of-line invariant
//! `pixel_x_at_byte(text.len()) == line.width_lpx`.
//!
//! Glyphs may render as `.notdef` when DejaVu Sans lacks a codepoint (e.g.
//! emoji, Hangul) but the shaper still emits well-defined cluster values, so
//! the geometry math under test is exercised regardless.

#![cfg(any(target_os = "windows", target_os = "macos"))]

use slate_text::{ShapedLine, TEST_FONT, TextBackend, byte_at_pixel_x, pixel_x_at_byte};
use unicode_segmentation::UnicodeSegmentation;

#[cfg(target_os = "macos")]
use slate_text::CoreTextBackend as BackendImpl;
#[cfg(target_os = "windows")]
use slate_text::DirectWriteBackend as BackendImpl;

fn shape(text: &str) -> ShapedLine {
    let mut backend = BackendImpl::new().expect("backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 1.0)
        .expect("font load");
    backend.shape_line(&font, text).expect("shape")
}

/// All grapheme boundaries in `text`, including 0 and `text.len()`.
fn grapheme_boundaries(text: &str) -> Vec<usize> {
    let mut out: Vec<usize> = text.grapheme_indices(true).map(|(b, _)| b).collect();
    if out.last().copied() != Some(text.len()) {
        out.push(text.len());
    }
    out
}

fn assert_roundtrip(label: &str, text: &str) {
    let line = shape(text);

    // End-of-line invariant.
    let end_x = pixel_x_at_byte(&line, text, text.len());
    assert!(
        (end_x - line.width_lpx).abs() < 0.01,
        "[{label}] pixel_x_at_byte(end)={end_x} != width_lpx={}",
        line.width_lpx,
    );

    // Per-boundary round trip.
    for b in grapheme_boundaries(text) {
        let x = pixel_x_at_byte(&line, text, b);
        let b2 = byte_at_pixel_x(&line, text, x);
        assert_eq!(
            b, b2,
            "[{label}] round-trip failed at byte {b} (x={x}, recovered {b2}); width={}",
            line.width_lpx,
        );
    }
}

#[test]
fn roundtrip_ascii() {
    assert_roundtrip("ascii", "abc");
}

#[test]
fn roundtrip_hiragana() {
    // Each kana is one grapheme; DejaVu lacks them so glyphs are .notdef but
    // clusters still come back correct.
    assert_roundtrip("hiragana", "こんにちは");
}

#[test]
fn roundtrip_hangul() {
    assert_roundtrip("hangul", "안녕하세요");
}

#[test]
fn roundtrip_regional_indicator_pairs() {
    // Two regional-indicator flags. Each flag is one Unicode grapheme even
    // though it's two codepoints.
    assert_roundtrip("flags", "\u{1F1EF}\u{1F1F5}\u{1F1FA}\u{1F1F8}");
}

#[test]
fn roundtrip_mixed_ascii_flag() {
    assert_roundtrip("mixed", "a\u{1F1EF}\u{1F1F5}b");
}

#[test]
fn roundtrip_zwj_family() {
    // 👨‍👩‍👧‍👦 — family ZWJ sequence: 7 codepoints, 1 grapheme.
    assert_roundtrip(
        "zwj_family",
        "f\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}g",
    );
}

#[test]
fn roundtrip_nfd_decomposed() {
    // "café" with NFD (combining acute on 'e'): 5 bytes, 4 graphemes.
    // The combining mark sits in the same cluster as its base, so the shaper
    // emits ≥2 glyphs sharing one cluster value — exercises the "trailing
    // zero-advance glyph in cluster" path through `pixel_x_at_byte`.
    assert_roundtrip("nfd", "cafe\u{0301}");
}

#[test]
fn caret_at_zero_and_end() {
    let text = "Hello";
    let line = shape(text);
    assert_eq!(byte_at_pixel_x(&line, text, -1.0), 0);
    assert_eq!(byte_at_pixel_x(&line, text, 0.0), 0);
    assert_eq!(byte_at_pixel_x(&line, text, line.width_lpx), text.len());
    assert_eq!(
        byte_at_pixel_x(&line, text, line.width_lpx + 100.0),
        text.len()
    );
}
