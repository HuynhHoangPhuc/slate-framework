//! Byte ↔ pixel-x geometry helpers for caret math on shaped lines.
//!
//! Replaces the `.chars().count() * width` MVP that breaks on CJK ligatures,
//! Indic clusters, and emoji ZWJ sequences. Both helpers are byte-keyed end to
//! end and snap to grapheme boundaries so callers never have to.
//!
//! **Direction model:** a [`ShapedLine`] with an empty `runs` list is a single
//! implicit LTR run (pure-LTR / CJK) — cluster values are non-decreasing in
//! visual (= logical) order, and the original linear pen walk handles it
//! byte-identically. When `runs` is populated (mixed / RTL text), the helpers
//! switch to the run-aware path: visual x is laid out left-to-right over the
//! level-runs in visual order, and within each run advances are summed forward
//! (LTR) or in reverse (RTL) so the caret tracks the correct visual edge of a
//! logical byte. A boundary byte (`runA.end == runB.start`) maps to two visual
//! x-positions; [`Affinity`] selects which — the run that *starts* there
//! (downstream, the default) or the run that *ends* there (upstream).

use crate::ShapedLine;
use crate::types::{Affinity, Direction, RunSpan};
use unicode_segmentation::UnicodeSegmentation;

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

/// Largest grapheme-cluster boundary `<= byte` in `text`. Clamped to
/// `[0, text.len()]`.
fn snap_grapheme_floor(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    let mut last = 0usize;
    for (b, _) in text.grapheme_indices(true) {
        if b > byte {
            return last;
        }
        last = b;
    }
    last
}

// ---------------------------------------------------------------------------
// Run-aware caret math (mixed-direction / RTL lines)
//
// Active when `line.runs` is non-empty. The shared coordinate space is the
// caller's: line-relative byte/cluster for TextField, document-absolute for
// TextArea. The functions never touch `text.len()` / `line.width_lpx` as LTR
// shortcuts — every value is derived from run order, glyph clusters, advances,
// and per-run direction — so they are correct for both coordinate spaces.
// ---------------------------------------------------------------------------

/// Summed advance of every glyph on `line` whose cluster falls inside `range`
/// (i.e. the on-screen width of one level-run).
fn run_width(line: &ShapedLine, range: &std::ops::Range<usize>) -> f32 {
    line.glyphs
        .iter()
        .filter(|g| range.contains(&(g.cluster as usize)))
        .map(|g| g.x_advance_lpx)
        .sum()
}

/// Visual x of the left edge of the run at visual index `vi` — the summed width
/// of every run before it in visual order.
fn run_x_start(line: &ShapedLine, vi: usize) -> f32 {
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
fn within_run_caret_x(line: &ShapedLine, run: &RunSpan, byte: usize) -> f32 {
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
fn owning_run_with_affinity(runs: &[RunSpan], byte: usize, affinity: Affinity) -> Option<usize> {
    match affinity {
        Affinity::Downstream => owning_run(runs, byte),
        Affinity::Upstream => runs
            .iter()
            .position(|r| r.byte_range.end == byte)
            .or_else(|| owning_run(runs, byte)),
    }
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

/// Run-aware hit-test for a run-bearing line: visual x → logical byte.
///
/// Picks the visual run whose x-span covers `x_lpx` (clamping to the first /
/// last run outside the line), then resolves the caret inside that run by the
/// midpoint rule over its candidate boundaries. Candidates are the run's
/// distinct cluster starts plus the run end; their within-run x is computed by
/// [`within_run_caret_x`], so the same rule handles LTR and RTL uniformly.
///
/// Public for the multi-line hit-test paths (see [`run_caret_x`]). No `text`
/// argument: candidate carets come from glyph clusters, which are already
/// grapheme boundaries.
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

/// Distinct cluster-start bytes within `range`, ascending. Multi-glyph clusters
/// collapse to one entry; these are the grapheme-aligned caret stops of a run.
fn run_cluster_starts(line: &ShapedLine, range: &std::ops::Range<usize>) -> Vec<usize> {
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
/// descend. A stop carries the [`Affinity`] that makes [`run_caret_x_at_affinity`]
/// render it at this same x (the run's *trailing* byte is `Upstream`, every other
/// stop is `Downstream`).
///
/// Equal-x stops at a direction seam are collapsed (Mac-style): the earlier of
/// the pair is dropped, keeping the next run's stop, so the caret always advances
/// visibly. Only a redundant rendering of an already-reachable byte is lost — no
/// logical byte becomes arrow-unreachable.
fn visual_caret_stops(line: &ShapedLine) -> Vec<(usize, Affinity, f32)> {
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
    use super::*;
    use crate::types::{FontId, ShapedGlyph};

    fn glyph(cluster: u32, adv: f32) -> ShapedGlyph {
        ShapedGlyph {
            glyph_id: 1,
            font_id: FontId::PRIMARY,
            font_handle: crate::FontHandle::default(),
            x_advance_lpx: adv,
            position_lpx: [0.0, 0.0],
            cluster,
            direction: crate::types::Direction::Ltr,
        }
    }

    fn line(glyphs: Vec<ShapedGlyph>) -> ShapedLine {
        let width: f32 = glyphs.iter().map(|g| g.x_advance_lpx).sum();
        ShapedLine {
            glyphs,
            width_lpx: width,
            ascent_lpx: 10.0,
            descent_lpx: -2.0,
            y_offset_lpx: 0.0,
            base_direction: crate::types::Direction::Ltr,
            runs: Vec::new(),
        }
    }

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
        for (b, _) in text.grapheme_indices(true) {
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

    // --- Run-aware (mixed-direction / RTL) coverage ---

    /// Glyph with an explicit run direction (RTL runs walk advances in reverse).
    fn dglyph(cluster: u32, adv: f32, dir: Direction) -> ShapedGlyph {
        ShapedGlyph {
            direction: dir,
            ..glyph(cluster, adv)
        }
    }

    /// Build a run-bearing line from glyphs (visual order) and runs (visual
    /// order). `width_lpx` is the glyph advance sum.
    fn run_line(glyphs: Vec<ShapedGlyph>, runs: Vec<RunSpan>) -> ShapedLine {
        let width: f32 = glyphs.iter().map(|g| g.x_advance_lpx).sum();
        ShapedLine {
            glyphs,
            width_lpx: width,
            ascent_lpx: 10.0,
            descent_lpx: -2.0,
            y_offset_lpx: 0.0,
            base_direction: Direction::Rtl,
            runs,
        }
    }

    fn run(range: std::ops::Range<usize>, dir: Direction) -> RunSpan {
        RunSpan {
            level: if dir == Direction::Rtl { 1 } else { 0 },
            byte_range: range,
            direction: dir,
        }
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
    fn mixed_ltr_rtl_hit_test_roundtrips_across_seam() {
        // Same layout as `mixed_ltr_rtl_caret_positions`: "ab" LTR (x 0..11) then
        // "אב" RTL (x 11..26). Every boundary byte must hit-test back to itself,
        // and a click clearly *inside* the LTR run must not jump to the boundary
        // byte (which is owned by the RTL run and renders a full run-width away).
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

    #[test]
    fn snap_floor_basics() {
        assert_eq!(snap_grapheme_floor("abc", 0), 0);
        assert_eq!(snap_grapheme_floor("abc", 1), 1);
        assert_eq!(snap_grapheme_floor("abc", 3), 3);
        assert_eq!(snap_grapheme_floor("abc", 999), 3);
        // "é" NFD: graphemes start at 0 only; byte 1, 2 snap to 0.
        let s = "e\u{0301}";
        assert_eq!(snap_grapheme_floor(s, 1), 0);
        assert_eq!(snap_grapheme_floor(s, 2), 0);
        assert_eq!(snap_grapheme_floor(s, 3), 3);
    }
}
