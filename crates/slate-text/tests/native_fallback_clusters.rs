//! Live-path proof that native shapers (CoreText / DirectWrite) handle
//! complex-script clusters and font fallback themselves.
//!
//! The Slate-side `fix_missing_glyphs`/`FontFallbackSystem` path was removed:
//! it had zero live callers and assumed a 1:1 glyph↔char mapping that complex
//! scripts violate. The real fallback path is `shape_line` → the platform
//! shaper substitutes a covering face per glyph and tags `ShapedGlyph::
//! font_handle`, which `run_builder` rasterizes against. These tests pin that:
//!
//! - covered codepoints never come back as `.notdef` (glyph_id 0) on the live
//!   path — the OS substitutes rather than dropping to tofu;
//! - cluster values stay monotonically non-decreasing in logical order, so the
//!   byte-keyed caret math holds even when glyph count ≠ char count;
//! - a codepoint the primary face lacks comes back tagged with a non-default
//!   `font_handle` — proof the substitute face was captured, not the primary.

#![cfg(any(target_os = "macos", target_os = "windows"))]

use slate_text::{FontHandle, ShapedLine, TEST_FONT, TextBackend};

/// Construct the platform backend, load the bundled DejaVu Sans primary face,
/// and shape `text` on the live `shape_line` path.
#[cfg(target_os = "macos")]
fn shape(text: &str) -> ShapedLine {
    use slate_text::CoreTextBackend;
    let mut backend = CoreTextBackend::new().expect("CoreText backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("load bundled DejaVu Sans");
    backend.shape_line(&font, text).expect("shape_line")
}

#[cfg(target_os = "windows")]
fn shape(text: &str) -> ShapedLine {
    use slate_text::DirectWriteBackend;
    let mut backend = DirectWriteBackend::new().expect("DirectWrite backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("load bundled DejaVu Sans");
    backend.shape_line(&font, text).expect("shape_line")
}

/// No glyph on the live path is `.notdef` for a string of covered codepoints —
/// the native shaper substitutes a covering face rather than emitting tofu.
fn assert_no_notdef(text: &str, label: &str) {
    let line = shape(text);
    assert!(
        !line.glyphs.is_empty(),
        "{label}: expected glyphs for non-empty input"
    );
    for (i, g) in line.glyphs.iter().enumerate() {
        assert_ne!(
            g.glyph_id, 0,
            "{label}: glyph {i} is .notdef — native substitution did not cover it",
        );
    }
}

/// Cluster values are sorted in one consistent direction across the glyph run:
/// ascending for an LTR run (glyph visual order = logical order), descending for
/// an RTL run (visual order = reverse of logical). A single `shape_line` call is
/// one resolved run, so its glyphs never interleave clusters from both
/// directions. Multi-glyph clusters (ligatures, conjuncts, marks) share a value.
/// The byte-keyed caret math in `glyph_geometry`/`multiline` depends on this
/// no-interleave invariant — it walks advances forward (LTR) or reverse (RTL)
/// within a run.
fn assert_clusters_monotonic(text: &str, label: &str) {
    let line = shape(text);
    let clusters: Vec<u32> = line.glyphs.iter().map(|g| g.cluster).collect();
    let non_decreasing = clusters.windows(2).all(|w| w[1] >= w[0]);
    let non_increasing = clusters.windows(2).all(|w| w[1] <= w[0]);
    assert!(
        non_decreasing || non_increasing,
        "{label}: clusters interleave directions (not sorted): {clusters:?}",
    );
}

#[test]
fn combining_mark_renders_without_tofu() {
    // "é" as e + U+0301 (NFD): two codepoints, one grapheme.
    assert_no_notdef("e\u{0301}", "combining acute");
    assert_clusters_monotonic("e\u{0301}", "combining acute");
}

#[test]
fn arabic_word_renders_without_tofu() {
    // "مرحبا" — Arabic contextual shaping (initial/medial/final forms).
    assert_no_notdef("مرحبا", "arabic word");
    assert_clusters_monotonic("مرحبا", "arabic word");
}

#[test]
fn devanagari_conjunct_renders_without_tofu() {
    // "क्ष" (ksha) — a Devanagari conjunct: 3 codepoints, fewer rendered forms.
    assert_no_notdef("क्ष", "devanagari conjunct");
    assert_clusters_monotonic("क्ष", "devanagari conjunct");
}

#[test]
fn zwj_emoji_renders_without_tofu() {
    // Family emoji: man + ZWJ + woman + ZWJ + girl — one grapheme, ZWJ-joined.
    assert_no_notdef(
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
        "zwj family emoji",
    );
    assert_clusters_monotonic(
        "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
        "zwj family emoji",
    );
}

/// A codepoint the primary (DejaVu Sans) face lacks comes back tagged with a
/// non-default `font_handle`: the native shaper substituted a covering face and
/// the shaping adapter captured it. This is the entire live fallback contract —
/// `run_builder` dispatches rasterization on exactly this handle.
#[test]
fn missing_codepoint_captures_substitute_handle() {
    // DejaVu Sans has no CJK or emoji coverage; the OS substitutes.
    let line = shape("世界");
    assert!(!line.glyphs.is_empty(), "expected glyphs for CJK input");
    let substituted = line
        .glyphs
        .iter()
        .any(|g| g.font_handle != FontHandle::default());
    assert!(
        substituted,
        "expected at least one glyph tagged with a substitute font_handle \
         (native fallback capture); none differed from the default sentinel",
    );
    // And still no tofu — the substitute actually covered the codepoints.
    for (i, g) in line.glyphs.iter().enumerate() {
        assert_ne!(g.glyph_id, 0, "glyph {i} is .notdef despite substitution");
    }
}
