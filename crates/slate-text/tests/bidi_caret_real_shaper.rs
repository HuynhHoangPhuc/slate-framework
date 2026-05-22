//! Live-path proof that the native shaper (CoreText / DirectWrite) drives the
//! run-aware BiDi caret geometry end to end.
//!
//! Every other geometry test in `glyph_geometry.rs` builds synthetic
//! hand-authored runs. These shape *real* Arabic through `shape_line_bidi` (the
//! same UAX #9 segment → shape → reorder pipeline the editor uses) and walk the
//! public visual-caret API on the result, so the run/direction/x invariants the
//! caret math depends on are verified against actual shaper output — not an
//! idealized model.
//!
//! Assertions are font-portable: they pin structural invariants (an RTL run
//! exists, the caret walks screen-rightward, the walk terminates at the visual
//! edge) and never exact glyph ids or advances, which differ across the macOS
//! and Windows shapers and across whatever face the OS substitutes when the
//! bundled DejaVu Sans lacks Arabic coverage.

#![cfg(any(target_os = "macos", target_os = "windows"))]

use slate_text::{
    Affinity, Direction, ShapedLine, TEST_FONT, TextBackend, run_caret_x_at_affinity,
    shape_line_bidi, visual_caret_step, visual_line_edge,
};

/// Construct the platform backend, load the bundled DejaVu Sans primary face,
/// and shape `text` through the BiDi pipeline (run-bearing line).
#[cfg(target_os = "macos")]
fn shape(text: &str) -> ShapedLine {
    use slate_text::CoreTextBackend;
    let mut backend = CoreTextBackend::new().expect("CoreText backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("load bundled DejaVu Sans");
    shape_line_bidi(&backend, &font, text).expect("shape_line_bidi")
}

#[cfg(target_os = "windows")]
fn shape(text: &str) -> ShapedLine {
    use slate_text::DirectWriteBackend;
    let mut backend = DirectWriteBackend::new().expect("DirectWrite backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("load bundled DejaVu Sans");
    shape_line_bidi(&backend, &font, text).expect("shape_line_bidi")
}

/// Shape a single segment with a forced RTL direction — the per-segment path
/// `shape_words_in` uses (including for whitespace runs between RTL words).
#[cfg(target_os = "macos")]
fn shape_rtl_segment(text: &str) -> ShapedLine {
    use slate_text::CoreTextBackend;
    let mut backend = CoreTextBackend::new().expect("CoreText backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("load bundled DejaVu Sans");
    backend
        .shape_segment(&font, text, Direction::Rtl)
        .expect("shape_segment")
}

#[cfg(target_os = "windows")]
fn shape_rtl_segment(text: &str) -> ShapedLine {
    use slate_text::DirectWriteBackend;
    let mut backend = DirectWriteBackend::new().expect("DirectWrite backend init");
    let font = backend
        .load_font_from_bytes(TEST_FONT, 16.0, 2.0)
        .expect("load bundled DejaVu Sans");
    backend
        .shape_segment(&font, text, Direction::Rtl)
        .expect("shape_segment")
}

/// Walk `visual_caret_step` rightward from the visual-left edge, returning the
/// ordered `(byte, affinity, x)` stops actually visited. Panics if the walk
/// fails to terminate (a stuck stop would loop forever).
fn walk_rightward(line: &ShapedLine) -> Vec<(usize, Affinity, f32)> {
    let (mut byte, mut aff) = visual_line_edge(line, false).expect("visual left edge");
    let mut visited = vec![(byte, aff, run_caret_x_at_affinity(line, byte, aff))];
    while let Some((b, a)) = visual_caret_step(line, byte, aff, true) {
        byte = b;
        aff = a;
        visited.push((byte, aff, run_caret_x_at_affinity(line, byte, aff)));
        assert!(visited.len() < 1000, "rightward caret walk did not terminate");
    }
    visited
}

#[test]
fn arabic_line_has_rtl_run_and_screen_monotonic_caret() {
    // "مرحبا" — pure Arabic, contextual shaping. The BiDi pipeline classifies it
    // as a single RTL level-run.
    let line = shape("مرحبا");
    assert!(!line.glyphs.is_empty(), "expected glyphs for Arabic input");
    assert!(
        !line.runs.is_empty(),
        "BiDi pipeline must populate runs for non-LTR text"
    );
    assert!(
        line.runs.iter().any(|r| r.direction == Direction::Rtl),
        "expected at least one RTL run, got {:?}",
        line.runs.iter().map(|r| r.direction).collect::<Vec<_>>()
    );

    let visited = walk_rightward(&line);
    assert!(
        visited.len() >= 2,
        "expected multiple caret stops for a 5-letter word, got {}",
        visited.len()
    );

    // Screen-rightward: x is non-decreasing across the walk (a small tolerance
    // absorbs sub-pixel rounding between the run-width and glyph-advance sums).
    for w in visited.windows(2) {
        assert!(
            w[1].2 >= w[0].2 - 0.05,
            "caret x must be non-decreasing (screen-rightward): {visited:?}"
        );
    }

    // Every press moves: no consecutive stop repeats the same (byte, affinity).
    for w in visited.windows(2) {
        assert!(
            (w[0].0, w[0].1) != (w[1].0, w[1].1),
            "caret stuck — duplicate stop in walk: {visited:?}"
        );
    }

    // The walk ends exactly at the visual right edge, then yields None past it.
    let (last_byte, last_aff, _) = *visited.last().unwrap();
    assert_eq!(
        visual_line_edge(&line, true),
        Some((last_byte, last_aff)),
        "rightward walk must terminate at the visual right edge"
    );
    assert_eq!(
        visual_caret_step(&line, last_byte, last_aff, true),
        None,
        "no stop past the visual right edge"
    );
}

#[test]
fn rtl_glyph_paint_positions_are_finite_and_word_origin_relative() {
    // Regression: the DirectWrite path created its layout with an f32::MAX
    // max-width. An RTL run anchors at the box's RIGHT edge, so glyph origins
    // came back at ~f32::MAX — where adding an advance is a float no-op (all
    // glyphs collapse onto one origin) and the paint rect overflowed to
    // infinity, rendering Arabic off-screen (blank) while caret math (which
    // uses advances, not positions) still passed. This pins the paint contract
    // `assemble_visual_line` depends on: every glyph's x is finite, the run is
    // word-origin-relative (min x == 0), positions are non-decreasing, and the
    // span stays within the advance-sum width. Font-portable (no glyph ids).
    let line = shape("مرحبا");
    assert!(!line.glyphs.is_empty(), "expected glyphs for Arabic input");

    let xs: Vec<f32> = line.glyphs.iter().map(|g| g.position_lpx[0]).collect();
    for &x in &xs {
        assert!(x.is_finite(), "glyph x must be finite, got {x} in {xs:?}");
    }
    let min_x = xs.iter().copied().fold(f32::INFINITY, f32::min);
    assert!(
        min_x.abs() < 0.05,
        "RTL run must be word-origin-relative (min x ≈ 0), got {min_x} in {xs:?}"
    );
    for w in xs.windows(2) {
        assert!(
            w[1] >= w[0] - 0.05,
            "glyph paint x must be non-decreasing in array order, got {xs:?}"
        );
    }
    let max_x = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    assert!(
        max_x <= line.width_lpx + 0.5,
        "glyph span ({max_x}) must stay within the run width ({})",
        line.width_lpx
    );
}

#[test]
fn rtl_whitespace_segment_glyphs_stay_finite_and_distinct() {
    // A pure-whitespace RTL segment (the space between Arabic words carries an
    // RTL level under UAX #9) reports a 0 natural width in DirectWrite's metrics.
    // Without a finite max-width fallback the layout keeps its f32::MAX box, the
    // space glyphs anchor at ~f32::MAX, their advances are swallowed by the
    // float grid, and post-Draw normalization then collapses them onto a single
    // origin. Drive the per-segment shaper directly (the path `shape_words_in`
    // takes for a space run) and assert the glyphs stay finite and spread.
    let line = shape_rtl_segment("  "); // two spaces, forced RTL
    assert!(!line.glyphs.is_empty(), "expected glyphs for the space run");
    let xs: Vec<f32> = line.glyphs.iter().map(|g| g.position_lpx[0]).collect();
    for &x in &xs {
        assert!(x.is_finite(), "space-glyph x must be finite, got {x} in {xs:?}");
    }
    if xs.len() >= 2 {
        // Distinct origins: a second space must not sit on top of the first.
        assert!(
            (xs[1] - xs[0]).abs() > 0.01,
            "multi-space RTL run collapsed onto one origin: {xs:?}"
        );
    }
}

#[test]
fn mixed_ltr_rtl_line_crosses_seam() {
    // "abمرحبا" — an LTR prefix then an Arabic (RTL) run: at least two level-runs
    // with a direction seam between them.
    let line = shape("abمرحبا");
    assert!(
        line.runs.len() >= 2,
        "mixed LTR+RTL text must yield ≥2 runs, got {}",
        line.runs.len()
    );
    assert!(
        line.runs.iter().any(|r| r.direction == Direction::Ltr)
            && line.runs.iter().any(|r| r.direction == Direction::Rtl),
        "expected both an LTR and an RTL run"
    );

    let visited = walk_rightward(&line);
    assert!(
        visited.len() >= 3,
        "expected several stops across the seam, got {}",
        visited.len()
    );

    // The rightward walk crosses the LTR↔RTL seam without a stuck stop: every
    // step changes (byte, affinity) — the seam-collapse rule guarantees no
    // back-and-forth or zero-width hop at the boundary.
    for w in visited.windows(2) {
        assert!(
            (w[0].0, w[0].1) != (w[1].0, w[1].1),
            "caret stuck at seam — duplicate stop: {visited:?}"
        );
    }
    for w in visited.windows(2) {
        assert!(
            w[1].2 >= w[0].2 - 0.05,
            "caret x must stay screen-rightward across the seam: {visited:?}"
        );
    }
}
