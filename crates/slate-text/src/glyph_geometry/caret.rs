//! Logical byte → visual caret x, run-aware caret motion, visual line edges,
//! and per-run selection rects.

use super::bidi_runs::{
    owning_run_with_affinity, run_x_start, visual_caret_stops, within_run_caret_x,
};
use super::shape_utils::snap_grapheme_floor;
use crate::ShapedLine;
use crate::types::Affinity;

/// Pen-x position (line-relative, lpx) of the leading edge of the cluster
/// whose first byte is at `byte` in `text`.
///
/// `byte` is snapped to the nearest leading grapheme boundary (floor). Byte 0
/// maps to 0.0; `byte >= text.len()` maps to `line.width_lpx`.
pub fn pixel_x_at_byte(line: &ShapedLine, text: &str, byte: usize) -> f32 {
    if line.glyphs.is_empty() {
        return 0.0;
    }
    if !line.runs.is_empty() {
        return run_caret_x(line, text, byte);
    }
    let snapped = snap_grapheme_floor(text, byte);
    if snapped == 0 {
        return 0.0;
    }
    if snapped >= text.len() {
        return line.width_lpx;
    }

    let mut pen = 0.0f32;
    for g in &line.glyphs {
        if (g.cluster as usize) >= snapped {
            return pen;
        }
        pen += g.x_advance_lpx;
    }
    line.width_lpx
}

/// Run-aware caret x for a run-bearing line. `byte` is snapped to a grapheme
/// floor in `text` (same coordinate space as the line's clusters).
///
/// Public so the multi-line caret paths (`multiline.rs`, TextArea layout) can
/// reuse the exact same run math on their document-absolute lines. Callers
/// guard on `!line.runs.is_empty()` and keep their own empty-runs fast path.
pub fn run_caret_x(line: &ShapedLine, text: &str, byte: usize) -> f32 {
    run_caret_x_at(line, snap_grapheme_floor(text, byte))
}

/// Run-aware caret x for a `byte` already known to be a grapheme boundary
/// (e.g. an editor caret offset), skipping the grapheme snap so callers that do
/// not carry the source text — like [`crate::multiline::MultilineLayout`] — can
/// reuse the run math.
pub fn run_caret_x_at(line: &ShapedLine, byte: usize) -> f32 {
    run_caret_x_at_affinity(line, byte, Affinity::Downstream)
}

/// Run-aware caret x for an already-grapheme-aligned `byte`, resolving a
/// direction-boundary byte to the visual edge that `affinity` selects.
///
/// At an LTR↔RTL boundary the same logical byte has two visual x-positions; this
/// returns the trailing edge of the run that *ends* at `byte` ([`Affinity::Upstream`])
/// or the leading edge of the run that *starts* there ([`Affinity::Downstream`]).
/// On a line with no direction boundary the result is independent of `affinity`.
pub fn run_caret_x_at_affinity(line: &ShapedLine, byte: usize, affinity: Affinity) -> f32 {
    match owning_run_with_affinity(&line.runs, byte, affinity) {
        Some(vi) => run_x_start(line, vi) + within_run_caret_x(line, &line.runs[vi], byte),
        None => 0.0,
    }
}

/// Visual selection rectangles for the logical byte range `[lo, hi)` on a
/// run-bearing line: one `(x_start_lpx, width_lpx)` per level-run the range
/// intersects.
///
/// A single logical range maps to one contiguous x-span *per run* but not across
/// runs (an RTL run reverses byte→x order, and visual run order need not follow
/// logical order), so a mixed-direction selection paints as several rectangles.
/// x is line-relative; callers offset by the line/element origin. Returns empty
/// for `lo >= hi` or a line with no runs (callers keep their own LTR fast path).
pub fn run_selection_rects(line: &ShapedLine, lo: usize, hi: usize) -> Vec<(f32, f32)> {
    let mut rects = Vec::new();
    if lo >= hi || line.runs.is_empty() {
        return rects;
    }
    for (vi, run) in line.runs.iter().enumerate() {
        let a = lo.max(run.byte_range.start);
        let b = hi.min(run.byte_range.end);
        if a >= b {
            continue;
        }
        let base = run_x_start(line, vi);
        let xa = base + within_run_caret_x(line, run, a);
        let xb = base + within_run_caret_x(line, run, b);
        // RTL reverses byte→x order, so the lower x can come from either end.
        let (x_start, x_end) = if xa <= xb { (xa, xb) } else { (xb, xa) };
        let width = x_end - x_start;
        if width > 0.0 {
            rects.push((x_start, width));
        }
    }
    rects
}

/// Visual (screen-direction) caret motion for a run-bearing line: step one caret
/// stop to the left (`move_right == false`) or right on screen, crossing run
/// boundaries. Returns the new `(byte, affinity)`, or `None` when the step would
/// move past the line's visual edge (the caller crosses to an adjacent line or
/// clamps).
///
/// `move_right == true` always moves rightward on screen regardless of the run's
/// direction — inside an RTL run that *decreases* the logical byte. On a line
/// with no direction boundary this reduces to logical grapheme motion.
pub fn visual_caret_step(
    line: &ShapedLine,
    byte: usize,
    affinity: Affinity,
    move_right: bool,
) -> Option<(usize, Affinity)> {
    let stops = visual_caret_stops(line);
    if stops.is_empty() {
        return None;
    }
    // Locate the current stop: prefer an exact (byte, affinity) match, else fall
    // back to the first stop with this byte (e.g. a click-placed caret whose
    // affinity defaulted, or a collapsed seam duplicate).
    let cur = stops
        .iter()
        .position(|&(b, a, _)| b == byte && a == affinity)
        .or_else(|| stops.iter().position(|&(b, _, _)| b == byte))?;
    let next = if move_right {
        cur.checked_add(1).filter(|&i| i < stops.len())
    } else {
        cur.checked_sub(1)
    }?;
    let (b, a, _) = stops[next];
    Some((b, a))
}

/// Visual line-edge caret stop for a run-bearing line: the leftmost stop
/// (`to_end == false`, Home) or the rightmost stop (`to_end == true`, End) on
/// screen, with its affinity. Returns `None` for a line with no runs — the
/// caller falls back to the logical line edges (`byte_start` / `line_caret_end`).
///
/// "Edge" is screen-relative, not logical: on a fully-RTL line the rightmost
/// stop is the logical line *start*. This matches the visual ←/→ model where
/// Home/End land where the caret would after pressing the arrow until it stops.
pub fn visual_line_edge(line: &ShapedLine, to_end: bool) -> Option<(usize, Affinity)> {
    let stops = visual_caret_stops(line);
    if to_end {
        stops.last().map(|&(b, a, _)| (b, a))
    } else {
        stops.first().map(|&(b, a, _)| (b, a))
    }
}

#[cfg(test)]
mod tests {
    use super::super::hit_test::byte_at_pixel_x;
    use super::super::test_support::*;
    use super::*;
    use crate::types::Direction;

    #[test]
    fn empty_line_returns_zero() {
        let l = line(vec![]);
        assert_eq!(pixel_x_at_byte(&l, "", 0), 0.0);
        assert_eq!(byte_at_pixel_x(&l, "", 0.0), 0);
    }

    #[test]
    fn ascii_boundaries_roundtrip() {
        // "abc" — 3 glyphs, clusters 0/1/2, advances 5/6/7.
        let l = line(vec![glyph(0, 5.0), glyph(1, 6.0), glyph(2, 7.0)]);
        assert_eq!(pixel_x_at_byte(&l, "abc", 0), 0.0);
        assert_eq!(pixel_x_at_byte(&l, "abc", 1), 5.0);
        assert_eq!(pixel_x_at_byte(&l, "abc", 2), 11.0);
        assert_eq!(pixel_x_at_byte(&l, "abc", 3), 18.0);
        assert_eq!(byte_at_pixel_x(&l, "abc", 0.0), 0);
        assert_eq!(byte_at_pixel_x(&l, "abc", 18.0), 3);
    }

    #[test]
    fn multi_glyph_cluster_decomposition() {
        // "é" encoded as e + combining acute (NFD): 3 bytes, 1 grapheme,
        // 2 glyphs both at cluster 0. Byte 3 (end) is the next boundary.
        let s = "e\u{0301}";
        let l = line(vec![glyph(0, 8.0), glyph(0, 0.0)]);
        assert_eq!(pixel_x_at_byte(&l, s, 0), 0.0);
        // Byte 1 snaps back to 0 (mid-grapheme): same pen-x as byte 0.
        assert_eq!(pixel_x_at_byte(&l, s, 1), 0.0);
        assert_eq!(pixel_x_at_byte(&l, s, 3), 8.0);
        // Round-trip on grapheme boundaries.
        assert_eq!(byte_at_pixel_x(&l, s, 0.0), 0);
        assert_eq!(byte_at_pixel_x(&l, s, 8.0), 3);
    }

    #[test]
    fn rtl_run_caret_walks_in_reverse() {
        // Pure-RTL run over a 9-byte, 3-grapheme string (3 bytes/char so the
        // grapheme boundaries land at 0/3/6/9, matching the synthetic clusters;
        // direction is forced on the glyphs/run, not inferred from the text).
        // Glyphs in visual (left-to-right) order: cluster 6, 3, 0 with advances
        // 7, 8, 9. Logical byte 0 is the rightmost edge.
        let glyphs = vec![
            dglyph(6, 7.0, Direction::Rtl),
            dglyph(3, 8.0, Direction::Rtl),
            dglyph(0, 9.0, Direction::Rtl),
        ];
        let l = run_line(glyphs, vec![run(0..9, Direction::Rtl)]);
        let text = "日本語";
        // byte 0 = logical start = right edge = full width (24).
        assert_eq!(pixel_x_at_byte(&l, text, 0), 24.0);
        // byte 3 = after first logical char (adv 9) → x = 24 - 9 = 15.
        assert_eq!(pixel_x_at_byte(&l, text, 3), 15.0);
        // byte 6 = after two chars → x = 24 - 9 - 8 = 7.
        assert_eq!(pixel_x_at_byte(&l, text, 6), 7.0);
        // byte 9 = logical end = left edge = 0.
        assert_eq!(pixel_x_at_byte(&l, text, 9), 0.0);
    }

    #[test]
    fn mixed_ltr_rtl_caret_positions() {
        // "ab" (LTR, bytes 0..2) then "אב" (RTL, bytes 2..6). Visual order is
        // LTR run first, then RTL run. LTR glyphs: cluster 0 adv 5, cluster 1
        // adv 6 (width 11). RTL glyphs (visual L→R): cluster 4 adv 7, cluster 2
        // adv 8 (run width 15). Total 26. RTL run starts at x=11.
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
        // LTR run, byte 0 → 0; byte 1 → 5; boundary byte 2 starts the RTL run.
        assert_eq!(pixel_x_at_byte(&l, text, 0), 0.0);
        assert_eq!(pixel_x_at_byte(&l, text, 1), 5.0);
        // byte 2 = RTL start = right edge of RTL run = 11 + 15 = 26.
        assert_eq!(pixel_x_at_byte(&l, text, 2), 26.0);
        // byte 4 = after first RTL char (adv 8) → 26 - 8 = 18.
        assert_eq!(pixel_x_at_byte(&l, text, 4), 18.0);
        // byte 6 = RTL end = left edge of RTL run = 11.
        assert_eq!(pixel_x_at_byte(&l, text, 6), 11.0);
    }

    #[test]
    fn visual_motion_pure_ltr_matches_logical() {
        // A single LTR run: visual right == logical next cluster, left == prev.
        let glyphs = vec![
            dglyph(0, 5.0, Direction::Ltr),
            dglyph(1, 6.0, Direction::Ltr),
            dglyph(2, 7.0, Direction::Ltr),
        ];
        let l = run_line(glyphs, vec![run(0..3, Direction::Ltr)]);
        assert_eq!(visual_caret_step(&l, 0, Affinity::Downstream, true), Some((1, Affinity::Downstream)));
        assert_eq!(visual_caret_step(&l, 1, Affinity::Downstream, true), Some((2, Affinity::Downstream)));
        assert_eq!(visual_caret_step(&l, 2, Affinity::Downstream, true), Some((3, Affinity::Upstream)));
        // Past the right edge → None (caller crosses line / clamps).
        assert_eq!(visual_caret_step(&l, 3, Affinity::Upstream, true), None);
        // Leftward.
        assert_eq!(visual_caret_step(&l, 3, Affinity::Upstream, false), Some((2, Affinity::Downstream)));
        assert_eq!(visual_caret_step(&l, 0, Affinity::Downstream, false), None);
    }

    #[test]
    fn visual_motion_pure_rtl_reverses_logical() {
        // Pure-RTL run "日本語" (bytes 0/3/6, end 9). Visual stops left→right are
        // logical-descending: 9, 6, 3, 0. Moving right on screen decreases byte.
        let glyphs = vec![
            dglyph(6, 7.0, Direction::Rtl),
            dglyph(3, 8.0, Direction::Rtl),
            dglyph(0, 9.0, Direction::Rtl),
        ];
        let l = run_line(glyphs, vec![run(0..9, Direction::Rtl)]);
        // Leftmost stop is byte 9 (logical end). Right → 6 → 3 → 0.
        assert_eq!(visual_caret_step(&l, 9, Affinity::Upstream, true), Some((6, Affinity::Downstream)));
        assert_eq!(visual_caret_step(&l, 6, Affinity::Downstream, true), Some((3, Affinity::Downstream)));
        assert_eq!(visual_caret_step(&l, 3, Affinity::Downstream, true), Some((0, Affinity::Downstream)));
        assert_eq!(visual_caret_step(&l, 0, Affinity::Downstream, true), None);
        // Leftward mirrors.
        assert_eq!(visual_caret_step(&l, 0, Affinity::Downstream, false), Some((3, Affinity::Downstream)));
        assert_eq!(visual_caret_step(&l, 9, Affinity::Upstream, false), None);
    }

    #[test]
    fn visual_motion_collapses_seam_mac_style() {
        // "ab" LTR (x 0..11) then "אב" RTL (x 11..26). Collapsed visual stops
        // left→right: (0,D,0) (1,D,5) (6,Up,11) (4,D,18) (2,D,26). The duplicate
        // (2,Up,11) is dropped — byte 2 stays reachable at x=26.
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
        // Rightward across the seam: 1 → 6 (RTL end, leftmost) → 4 → 2 (RTL start).
        assert_eq!(visual_caret_step(&l, 1, Affinity::Downstream, true), Some((6, Affinity::Upstream)));
        assert_eq!(visual_caret_step(&l, 6, Affinity::Upstream, true), Some((4, Affinity::Downstream)));
        assert_eq!(visual_caret_step(&l, 4, Affinity::Downstream, true), Some((2, Affinity::Downstream)));
        assert_eq!(visual_caret_step(&l, 2, Affinity::Downstream, true), None);
        // Leftward: 2 → 4 → 6 → 1 → 0.
        assert_eq!(visual_caret_step(&l, 2, Affinity::Downstream, false), Some((4, Affinity::Downstream)));
        assert_eq!(visual_caret_step(&l, 6, Affinity::Upstream, false), Some((1, Affinity::Downstream)));
        // The dropped duplicate (2, Upstream) still resolves by byte fallback.
        assert_eq!(visual_caret_step(&l, 2, Affinity::Upstream, false), Some((4, Affinity::Downstream)));
    }

    #[test]
    fn selection_rects_split_at_direction_boundary() {
        // "ab" LTR (x 0..11) then "אב" RTL (x 11..26). Selecting bytes [1,4)
        // covers "b" (LTR, x 5..11) and the first logical RTL char (bytes 2..4),
        // which renders at the RTL run's right portion (x 18..26). Two rects.
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
        let rects = run_selection_rects(&l, 1, 4);
        assert_eq!(rects.len(), 2);
        // LTR part: x 5..11 → (5, 6).
        assert_eq!(rects[0], (5.0, 6.0));
        // RTL part bytes [2,4): within_rtl(2)=15 (x26), within_rtl(4)=7 (x18) →
        // rect (18, 8).
        assert_eq!(rects[1], (18.0, 8.0));
        // Empty / inverted range → no rects.
        assert!(run_selection_rects(&l, 3, 3).is_empty());
    }

    #[test]
    fn boundary_byte_resolves_per_affinity() {
        // "ab" LTR (x 0..11) then "אב" RTL (x 11..26). Byte 2 is the LTR↔RTL
        // boundary: it ends the LTR run and starts the RTL run.
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
        // Upstream → trailing edge of the LTR run that ends at byte 2 (x=11).
        assert_eq!(run_caret_x_at_affinity(&l, 2, Affinity::Upstream), 11.0);
        // Downstream → leading edge of the RTL run that starts at byte 2 = its
        // visual right edge (x=26). Matches the default `run_caret_x_at`.
        assert_eq!(run_caret_x_at_affinity(&l, 2, Affinity::Downstream), 26.0);
        assert_eq!(run_caret_x_at(&l, 2), 26.0);
        // An interior byte (4, inside the RTL run) is affinity-independent.
        assert_eq!(
            run_caret_x_at_affinity(&l, 4, Affinity::Upstream),
            run_caret_x_at_affinity(&l, 4, Affinity::Downstream),
        );
    }

    #[test]
    fn visual_line_edges_are_screen_leftmost_and_rightmost() {
        // "ab" LTR (x 0..11) then "אב" RTL (x 11..26). Collapsed stops left→right:
        // (0,D,0) (1,D,5) (6,Up,11) (4,D,18) (2,D,26). Home = leftmost = (0,D),
        // End = rightmost = (2,D) — note byte 2 is the *logical* RTL start.
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
        assert_eq!(visual_line_edge(&l, false), Some((0, Affinity::Downstream)));
        assert_eq!(visual_line_edge(&l, true), Some((2, Affinity::Downstream)));
        // Pure-RTL line: leftmost is the logical end (byte 9), rightmost byte 0.
        let rtl = run_line(
            vec![
                dglyph(6, 7.0, Direction::Rtl),
                dglyph(3, 8.0, Direction::Rtl),
                dglyph(0, 9.0, Direction::Rtl),
            ],
            vec![run(0..9, Direction::Rtl)],
        );
        assert_eq!(visual_line_edge(&rtl, false), Some((9, Affinity::Upstream)));
        assert_eq!(visual_line_edge(&rtl, true), Some((0, Affinity::Downstream)));
        // A line with no runs has no visual stops.
        assert_eq!(visual_line_edge(&line(vec![glyph(0, 5.0)]), false), None);
    }

    #[test]
    fn empty_runs_path_is_unchanged_by_run_branch() {
        // Regression: a line with empty `runs` must take the original LTR pen
        // walk, byte-for-byte. (Same asserts as `ascii_boundaries_roundtrip`.)
        let l = line(vec![glyph(0, 5.0), glyph(1, 6.0), glyph(2, 7.0)]);
        assert!(l.runs.is_empty());
        assert_eq!(pixel_x_at_byte(&l, "abc", 0), 0.0);
        assert_eq!(pixel_x_at_byte(&l, "abc", 1), 5.0);
        assert_eq!(pixel_x_at_byte(&l, "abc", 2), 11.0);
        assert_eq!(pixel_x_at_byte(&l, "abc", 3), 18.0);
        assert_eq!(byte_at_pixel_x(&l, "abc", 0.0), 0);
        assert_eq!(byte_at_pixel_x(&l, "abc", 5.0), 1);
        assert_eq!(byte_at_pixel_x(&l, "abc", 18.0), 3);
    }
}
