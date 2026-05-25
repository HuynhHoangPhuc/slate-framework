//! Run-aware geometry primitives (run width, run-start x, within-run caret x,
//! ownership, visual caret-stop assembly) shared by the caret and hit-test
//! paths for mixed-direction / pure-RTL lines.
//!
//! The shared coordinate space is the caller's: line-relative byte/cluster for
//! TextField, document-absolute for TextArea. The functions never touch
//! `text.len()` / `line.width_lpx` as LTR shortcuts — every value is derived
//! from run order, glyph clusters, advances, and per-run direction — so they
//! are correct for both coordinate spaces.

use crate::ShapedLine;
use crate::types::{Affinity, Direction, RunSpan};

/// Summed advance of every glyph on `line` whose cluster falls inside `range`
/// (i.e. the on-screen width of one level-run).
pub(super) fn run_width(line: &ShapedLine, range: &std::ops::Range<usize>) -> f32 {
    line.glyphs
        .iter()
        .filter(|g| range.contains(&(g.cluster as usize)))
        .map(|g| g.x_advance_lpx)
        .sum()
}

/// Visual x of the left edge of the run at visual index `vi` — the summed width
/// of every run before it in visual order.
pub(super) fn run_x_start(line: &ShapedLine, vi: usize) -> f32 {
    line.runs[..vi]
        .iter()
        .map(|r| run_width(line, &r.byte_range))
        .sum()
}

/// Caret offset (from the run's left edge) of logical `byte` within `run`.
///
/// LTR: sum advances of glyphs logically *before* `byte` (cluster < byte). RTL:
/// sum advances of glyphs at-or-after `byte` (cluster >= byte) — the caret's
/// visual position grows leftward with logical progress, so the leading edge of
/// a byte sits to the right of every later-logical glyph.
pub(super) fn within_run_caret_x(line: &ShapedLine, run: &RunSpan, byte: usize) -> f32 {
    line.glyphs
        .iter()
        .filter(|g| run.byte_range.contains(&(g.cluster as usize)))
        .filter(|g| match run.direction {
            Direction::Ltr => (g.cluster as usize) < byte,
            Direction::Rtl => (g.cluster as usize) >= byte,
        })
        .map(|g| g.x_advance_lpx)
        .sum()
}

/// Visual index of the run owning logical `byte` under [`Affinity::Downstream`]:
/// the run that *starts* at a boundary byte wins (`start <= byte < end`); the
/// text-end byte (no run starts there) is owned by the run whose range *ends*
/// there.
fn owning_run(runs: &[RunSpan], byte: usize) -> Option<usize> {
    runs.iter()
        .position(|r| r.byte_range.start <= byte && byte < r.byte_range.end)
        .or_else(|| runs.iter().position(|r| r.byte_range.end == byte))
}

/// Visual index of the run owning logical `byte` under `affinity`.
///
/// At a direction boundary (`byte` is both `runA.end` and `runB.start`) the two
/// affinities pick different runs: `Downstream` the run starting at `byte`,
/// `Upstream` the run ending there. For an interior byte (only one run contains
/// it) and for the text-end byte both affinities resolve to the same run.
pub(super) fn owning_run_with_affinity(
    runs: &[RunSpan],
    byte: usize,
    affinity: Affinity,
) -> Option<usize> {
    match affinity {
        Affinity::Downstream => owning_run(runs, byte),
        Affinity::Upstream => runs
            .iter()
            .position(|r| r.byte_range.end == byte)
            .or_else(|| owning_run(runs, byte)),
    }
}

/// Distinct cluster-start bytes within `range`, ascending. Multi-glyph clusters
/// collapse to one entry; these are the grapheme-aligned caret stops of a run.
pub(super) fn run_cluster_starts(
    line: &ShapedLine,
    range: &std::ops::Range<usize>,
) -> Vec<usize> {
    let mut starts: Vec<usize> = line
        .glyphs
        .iter()
        .map(|g| g.cluster as usize)
        .filter(|c| range.contains(c))
        .collect();
    starts.sort_unstable();
    starts.dedup();
    starts
}

/// Ordered visual caret stops of a run-bearing line: `(byte, affinity, x_lpx)`
/// left-to-right by screen x.
///
/// Each run contributes a stop at every grapheme-aligned cluster start plus its
/// end byte; within an LTR run the bytes ascend with x, within an RTL run they
/// descend. A stop carries the [`Affinity`] that makes
/// `run_caret_x_at_affinity` render it at this same x (the run's *trailing*
/// byte is `Upstream`, every other stop is `Downstream`).
///
/// Equal-x stops at a direction seam are collapsed (Mac-style): the earlier of
/// the pair is dropped, keeping the next run's stop, so the caret always advances
/// visibly. Only a redundant rendering of an already-reachable byte is lost — no
/// logical byte becomes arrow-unreachable.
pub(super) fn visual_caret_stops(line: &ShapedLine) -> Vec<(usize, Affinity, f32)> {
    let mut stops: Vec<(usize, Affinity, f32)> = Vec::new();
    for (vi, run) in line.runs.iter().enumerate() {
        let base = run_x_start(line, vi);
        let mut bytes = run_cluster_starts(line, &run.byte_range);
        bytes.push(run.byte_range.end);
        bytes.dedup();
        // LTR: ascending byte = ascending x. RTL: ascending byte = descending x,
        // so reverse to emit left-to-right.
        if run.direction == Direction::Rtl {
            bytes.reverse();
        }
        for b in bytes {
            let affinity = if b == run.byte_range.end {
                Affinity::Upstream
            } else {
                Affinity::Downstream
            };
            stops.push((b, affinity, base + within_run_caret_x(line, run, b)));
        }
    }
    // Runs are laid out left-to-right, so `stops` is already x-ascending. Collapse
    // adjacent equal-x pairs by dropping the earlier (keep the next run's stop).
    // The two seam stops are summed by different paths (`run_x_start` over run
    // widths vs `within_run_caret_x` over glyph advances), so they can differ by
    // accumulated float rounding — compare with a sub-pixel tolerance, not the
    // near-1.0 `f32::EPSILON`, which never fires at realistic x magnitudes.
    let mut collapsed: Vec<(usize, Affinity, f32)> = Vec::with_capacity(stops.len());
    for s in stops {
        if let Some(last) = collapsed.last()
            && (last.2 - s.2).abs() < SEAM_COLLAPSE_TOLERANCE_LPX
        {
            collapsed.pop();
        }
        collapsed.push(s);
    }
    collapsed
}

/// Sub-pixel tolerance for treating two caret stops as the same seam point.
/// Far below one logical pixel (so genuinely distinct stops, which differ by a
/// whole glyph advance, never merge) yet far above accumulated float rounding.
const SEAM_COLLAPSE_TOLERANCE_LPX: f32 = 0.05;

#[cfg(test)]
mod tests {
    use super::super::caret::visual_caret_step;
    use super::super::test_support::*;
    use super::*;

    #[test]
    fn seam_collapse_threshold_holds_at_realistic_dpi() {
        // The 0.05 lpx tolerance must still merge a true seam coincidence — and
        // still refuse to merge genuinely distinct stops — at the x magnitudes a
        // 2× / ~192-DPI display produces (x ≈ 300), where the original
        // `f32::EPSILON` test silently stopped firing: one ULP near x=300 is
        // ≈3e-5, dwarfing EPSILON ≈1.2e-7, so coincident-but-rounded stops never
        // collapsed and the caret double-stepped at the seam.

        // (a) Real LTR→RTL seam at x ≈ 300: "ab" LTR scaled to width 300, then a
        // small RTL run. The LTR-end stop and the RTL run's leftmost stop both
        // land at x = 300 and collapse to one, exactly as at small magnitudes.
        let mixed = run_line(
            vec![
                dglyph(0, 150.0, Direction::Ltr),
                dglyph(1, 150.0, Direction::Ltr),
                dglyph(4, 7.0, Direction::Rtl),
                dglyph(2, 8.0, Direction::Rtl),
            ],
            vec![run(0..2, Direction::Ltr), run(2..6, Direction::Rtl)],
        );
        // Six raw stops; the duplicate seam pair at x=300 collapses to five.
        assert_eq!(visual_caret_stops(&mixed).len(), 5);
        // The dropped duplicate's byte stays reachable (byte 2 lives at the RTL
        // right edge); stepping right from byte 1 lands on the kept seam stop.
        assert_eq!(
            visual_caret_step(&mixed, 1, Affinity::Downstream, true),
            Some((6, Affinity::Upstream))
        );

        // (b) Near-threshold pair at x ≈ 300: two LTR stops 0.03 lpx apart (below
        // 0.05) merge. 0.03 is ~250000× f32::EPSILON, so an EPSILON check would
        // wrongly keep both — this pins the tolerance as deliberately sub-pixel.
        let near = run_line(
            vec![
                dglyph(0, 150.0, Direction::Ltr),
                dglyph(1, 150.0, Direction::Ltr),
                dglyph(2, 0.03, Direction::Ltr),
            ],
            vec![run(0..3, Direction::Ltr)],
        );
        // Stops at x = 0, 150, 300, 300.03 → last pair (Δ0.03 < 0.05) collapses.
        assert_eq!(visual_caret_stops(&near).len(), 3);

        // (c) A real glyph advance apart (Δ7 ≫ 0.05) at x ≈ 300 must NOT merge —
        // the tolerance sits far below one logical pixel, so distinct stops survive.
        let distinct = run_line(
            vec![
                dglyph(0, 150.0, Direction::Ltr),
                dglyph(1, 150.0, Direction::Ltr),
                dglyph(2, 7.0, Direction::Ltr),
            ],
            vec![run(0..3, Direction::Ltr)],
        );
        // Stops at x = 0, 150, 300, 307 → all four kept.
        assert_eq!(visual_caret_stops(&distinct).len(), 4);
    }
}
