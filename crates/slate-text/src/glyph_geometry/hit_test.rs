//! Visual x → logical byte hit-test for LTR and run-bearing lines.

use super::bidi_runs::{run_width, within_run_caret_x};
use crate::ShapedLine;
use unicode_segmentation::UnicodeSegmentation;

/// Byte offset of the grapheme boundary nearest `x_lpx` (line-relative).
///
/// Mid-cluster x rounds to the leading/trailing edge by midpoint test:
/// `x_lpx < midpoint → leading boundary; else trailing boundary`.
/// `x_lpx <= 0` returns 0; `x_lpx >= line.width_lpx` returns `text.len()`.
pub fn byte_at_pixel_x(line: &ShapedLine, text: &str, x_lpx: f32) -> usize {
    if !line.runs.is_empty() {
        return run_byte_at_x(line, x_lpx);
    }
    if x_lpx <= 0.0 {
        return 0;
    }
    if x_lpx >= line.width_lpx {
        return text.len();
    }

    // Two sequential linear passes (O(N+G)), no nested glyph walk.
    //
    // Pass 1: build a byte-sorted table of (cluster_byte, pen_x) recording the
    // pen-x at the leading edge of each cluster. Clusters are non-decreasing in
    // LTR order, so an entry is pushed only when the cluster value increases;
    // pen_x is the running sum of advances of all earlier-cluster glyphs.
    let mut cluster_pen: Vec<(usize, f32)> = Vec::with_capacity(line.glyphs.len());
    let mut pen = 0.0f32;
    for g in &line.glyphs {
        let c = g.cluster as usize;
        match cluster_pen.last() {
            Some(&(last_c, _)) if last_c == c => {}
            _ => cluster_pen.push((c, pen)),
        }
        pen += g.x_advance_lpx;
    }

    // Pen-x at a grapheme boundary `byte`: the stored pen of the first cluster
    // entry whose cluster value is `>= byte` — i.e. the running advance over all
    // glyphs with `cluster < byte`, matching `pixel_x_at_byte`'s `cluster >=
    // snapped` early-return. If no cluster reaches `byte`, the boundary is past
    // every glyph → `line.width_lpx`. `cursor` advances monotonically across the
    // grapheme walk since both tables are byte-sorted → amortized O(1).
    let mut cursor = 0usize;
    let pen_at = |byte: usize, cursor: &mut usize| -> f32 {
        while *cursor < cluster_pen.len() && cluster_pen[*cursor].0 < byte {
            *cursor += 1;
        }
        if *cursor < cluster_pen.len() {
            cluster_pen[*cursor].1
        } else {
            line.width_lpx
        }
    };

    // Pass 2: merge-walk grapheme boundaries, applying the midpoint rule.
    let mut last_b = 0usize;
    let mut last_x = 0.0f32;
    for (b, _) in text.grapheme_indices(true) {
        if b == 0 {
            continue;
        }
        let x = pen_at(b, &mut cursor);
        if x_lpx < x {
            let mid = (last_x + x) * 0.5;
            return if x_lpx < mid { last_b } else { b };
        }
        last_b = b;
        last_x = x;
    }
    // Past the last interior boundary: split between last_b and text.len().
    let mid = (last_x + line.width_lpx) * 0.5;
    if x_lpx < mid { last_b } else { text.len() }
}

/// Run-aware hit-test for a run-bearing line: visual x → logical byte.
///
/// Picks the visual run whose x-span covers `x_lpx` (clamping to the first /
/// last run outside the line), then resolves the caret inside that run by the
/// midpoint rule over its candidate boundaries. Candidates are the run's
/// distinct cluster starts plus the run end; their within-run x is computed by
/// [`within_run_caret_x`], so the same rule handles LTR and RTL uniformly.
///
/// Public for the multi-line hit-test paths. No `text` argument: candidate
/// carets come from glyph clusters, which are already grapheme boundaries.
pub fn run_byte_at_x(line: &ShapedLine, x_lpx: f32) -> usize {
    if line.runs.is_empty() || line.glyphs.is_empty() {
        return 0;
    }

    // Locate the owning run and its left-edge x (`acc` = total width of runs
    // before it). The last run absorbs any x at or beyond the line's right edge.
    let last = line.runs.len() - 1;
    let mut vi = last;
    let mut start_x = 0.0f32;
    let mut acc = 0.0f32;
    for (i, r) in line.runs.iter().enumerate() {
        let w = run_width(line, &r.byte_range);
        if x_lpx < acc + w || i == last {
            vi = i;
            start_x = acc;
            break;
        }
        acc += w;
    }
    let run = &line.runs[vi];
    let local = x_lpx - start_x;

    // Candidate caret bytes within the run: each distinct cluster start, plus
    // the run end *only* when it is the line-final boundary. A shared boundary
    // byte (some other run starts there) is owned by that next run — its caret
    // lives in the next run's coordinate space — so offering it here would let a
    // click inside this run resolve to a byte that renders a full run-width
    // away, breaking the hit-test ↔ caret round-trip at a direction seam.
    let mut cands: Vec<(f32, usize)> = Vec::new();
    let mut last_cluster: Option<usize> = None;
    for g in line.glyphs.iter() {
        let c = g.cluster as usize;
        if !run.byte_range.contains(&c) || last_cluster == Some(c) {
            continue;
        }
        last_cluster = Some(c);
        cands.push((within_run_caret_x(line, run, c), c));
    }
    let end = run.byte_range.end;
    let end_is_shared = line.runs.iter().any(|r| r.byte_range.start == end);
    if !end_is_shared {
        cands.push((within_run_caret_x(line, run, end), end));
    }
    if cands.is_empty() {
        return run.byte_range.start;
    }
    cands.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    for w in cands.windows(2) {
        let (x0, b0) = w[0];
        let (x1, b1) = w[1];
        if local < x1 {
            let mid = (x0 + x1) * 0.5;
            return if local < mid { b0 } else { b1 };
        }
    }
    cands.last().map(|&(_, b)| b).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::super::caret::pixel_x_at_byte;
    use super::super::test_support::*;
    use super::*;
    use crate::ShapedLine;
    use crate::types::{Direction, ShapedGlyph};

    #[test]
    fn midpoint_rounding() {
        // Two glyphs, advance 10 each: 'a' (cluster 0), 'b' (cluster 1).
        let l = line(vec![glyph(0, 10.0), glyph(1, 10.0)]);
        // x < 5 → 0; x ≥ 5 → 1 (midpoint of first cluster).
        assert_eq!(byte_at_pixel_x(&l, "ab", 4.9), 0);
        assert_eq!(byte_at_pixel_x(&l, "ab", 5.0), 1);
        // x < 15 → 1; x ≥ 15 → 2 (midpoint of second cluster).
        assert_eq!(byte_at_pixel_x(&l, "ab", 14.9), 1);
        assert_eq!(byte_at_pixel_x(&l, "ab", 15.0), 2);
    }

    /// Reference impl of `byte_at_pixel_x` using the old per-boundary
    /// `pixel_x_at_byte` walk. Kept local to the test module so the O(N²)
    /// semantics stay verifiable after the production path was rewritten.
    fn byte_at_pixel_x_reference(line: &ShapedLine, text: &str, x_lpx: f32) -> usize {
        if x_lpx <= 0.0 {
            return 0;
        }
        if x_lpx >= line.width_lpx {
            return text.len();
        }
        let mut last_b = 0usize;
        let mut last_x = 0.0f32;
        for (b, _) in unicode_segmentation::UnicodeSegmentation::grapheme_indices(text, true) {
            if b == 0 {
                continue;
            }
            let x = pixel_x_at_byte(line, text, b);
            if x_lpx < x {
                let mid = (last_x + x) * 0.5;
                return if x_lpx < mid { last_b } else { b };
            }
            last_b = b;
            last_x = x;
        }
        let mid = (last_x + line.width_lpx) * 0.5;
        if x_lpx < mid { last_b } else { text.len() }
    }

    #[test]
    fn large_input_agreement() {
        // ~500 ASCII glyphs, ascending clusters, varied advances. Sweep x
        // across the whole line and assert the rewritten O(N+G) impl matches
        // the old per-boundary reference for every sample.
        let n = 500usize;
        let text: String = "a".repeat(n);
        let glyphs: Vec<ShapedGlyph> = (0..n)
            .map(|i| glyph(i as u32, 3.0 + (i % 7) as f32))
            .collect();
        let l = line(glyphs);

        let mut x = -5.0f32;
        while x <= l.width_lpx + 5.0 {
            assert_eq!(
                byte_at_pixel_x(&l, &text, x),
                byte_at_pixel_x_reference(&l, &text, x),
                "mismatch at x={x}"
            );
            x += 0.37;
        }
    }

    #[test]
    fn rtl_hit_test_roundtrips() {
        let glyphs = vec![
            dglyph(6, 7.0, Direction::Rtl),
            dglyph(3, 8.0, Direction::Rtl),
            dglyph(0, 9.0, Direction::Rtl),
        ];
        let l = run_line(glyphs, vec![run(0..9, Direction::Rtl)]);
        let text = "日本語";
        for &byte in &[0usize, 3, 6, 9] {
            let x = pixel_x_at_byte(&l, text, byte);
            assert_eq!(
                byte_at_pixel_x(&l, text, x),
                byte,
                "x={x} should round-trip to byte {byte}"
            );
        }
        // Far left → logical end (9); far right → logical start (0).
        assert_eq!(byte_at_pixel_x(&l, text, -100.0), 9);
        assert_eq!(byte_at_pixel_x(&l, text, 1000.0), 0);
    }

    #[test]
    fn mixed_ltr_rtl_hit_test_roundtrips_across_seam() {
        // "ab" LTR (x 0..11) then "אב" RTL (x 11..26). Every boundary byte must
        // hit-test back to itself, and a click clearly *inside* the LTR run must
        // not jump to the boundary byte (which is owned by the RTL run and
        // renders a full run-width away).
        let glyphs = vec![
            dglyph(0, 5.0, Direction::Ltr),
            dglyph(1, 6.0, Direction::Ltr),
            dglyph(4, 7.0, Direction::Rtl),
            dglyph(2, 8.0, Direction::Rtl),
        ];
        let l = run_line(
            glyphs,
            vec![run(0..2, Direction::Ltr), run(2..6, Direction::Rtl)],
        );
        let text = "abאב";
        for &byte in &[0usize, 1, 2, 4, 6] {
            let x = pixel_x_at_byte(&l, text, byte);
            assert_eq!(
                byte_at_pixel_x(&l, text, x),
                byte,
                "x={x} should round-trip to byte {byte}"
            );
        }
        // A click at x=10 (inside the LTR run, past byte 1) resolves within the
        // LTR run, never to the boundary byte 2 that lives at x=26.
        assert_eq!(byte_at_pixel_x(&l, text, 10.0), 1);
    }
}
