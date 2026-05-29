//! Greedy first-fit wrap and visual-line assembly.
//!
//! Consumes pre-shaped [`ShapedWord`] items from [`super::shape`] and produces
//! [`ShapedLine`]s by pure width arithmetic — no shaping calls. Per line, items
//! are reordered into visual order via UAX #9 rule L2 and tab stops are snapped
//! against the running pen.

use crate::bidi;
use crate::types::{Direction, RunSpan, ShapedGlyph, ShapedLine};

use super::shape::ShapedWord;

/// Fit pre-shaped items into lines by greedy first-fit — pure width arithmetic,
/// no shaping calls.
///
/// Items (text words + ASCII-space runs from [`shape_words`]) are concatenated
/// at a running pen with no implicit inter-word gap — the gap is now an
/// explicit space-run item carrying one glyph per space byte. A space run is a
/// soft-break candidate: when a following word overflows the line, the pending
/// run is *absorbed* (dropped, contributing no visible width) and the word
/// starts the next line; a run that reaches the end of the text is kept
/// (visible) so trailing/standalone spaces stay addressable. The first text
/// word on a line is always accepted even if it overflows `max_width_lpx`.
/// `space_width` is accepted for API stability but no longer drives the join.
/// `line_height` is written into each line's `y_offset_lpx`.
///
/// Unlike the multi-line fit, this single-line path has no over-wide-word
/// break, so a leading space run before an over-wide first word stays visible
/// here (the multi-line fit absorbs it). Harmless: this path feeds non-editable
/// `Text` rendering only and does no caret/byte math on these glyphs.
///
/// [`shape_words`]: super::shape_words
pub fn wrap_shaped_words(
    items: &[ShapedWord],
    space_width: f32,
    line_height: f32,
    max_width_lpx: f32,
) -> Vec<ShapedLine> {
    let tab_width = 4.0 * space_width;
    let mut lines: Vec<ShapedLine> = Vec::new();
    // Items committed to the current line in logical (source) order; glyphs are
    // placed in visual order only at line flush (`assemble_visual_line`).
    let mut cur: Vec<&ShapedWord> = Vec::new();
    let mut cur_width = 0.0f32;
    let mut cur_ascent = 0.0f32;
    let mut cur_descent = 0.0f32;
    let mut y_offset = 0.0f32;
    // A trailing space run not yet committed to the line's visible width.
    let mut pending: Option<&ShapedWord> = None;

    for item in items {
        if item.is_space_run {
            // Hold the run: it is either committed before a following item or
            // absorbed at a wrap. (Scanner never emits two runs in a row.)
            pending = Some(item);
            continue;
        }

        let pending_w = pending.map(|s| s.advance_width_lpx).unwrap_or(0.0);
        let item_w = fit_advance(item, cur_width + pending_w, tab_width);
        let candidate = if cur.is_empty() {
            item_w
        } else {
            cur_width + pending_w + item_w
        };

        // Wrap only at an allowed UAX #14 opportunity: the item must overflow,
        // not be alone on the line, and carry a break before it. Items without
        // `break_before` (no break opportunity) ride out the overflow.
        if candidate > max_width_lpx && !cur.is_empty() && item.break_before {
            // Absorb the pending space run, flush, start the item's line.
            lines.push(assemble_visual_line(
                &std::mem::take(&mut cur),
                cur_ascent,
                cur_descent,
                y_offset,
                false,
                tab_width,
            ));
            y_offset += line_height;
            cur_width = 0.0;
            pending = None;
        } else if let Some(sp) = pending.take() {
            // Item fits with the run: make the spaces visible, then the item.
            commit_item(
                sp,
                sp.advance_width_lpx,
                &mut cur,
                &mut cur_width,
                &mut cur_ascent,
                &mut cur_descent,
            );
        }

        let placed_w = fit_advance(item, cur_width, tab_width);
        commit_item(
            item,
            placed_w,
            &mut cur,
            &mut cur_width,
            &mut cur_ascent,
            &mut cur_descent,
        );
    }

    // Trailing/standalone spaces at the end of the text stay visible.
    if let Some(sp) = pending.take() {
        commit_item(
            sp,
            sp.advance_width_lpx,
            &mut cur,
            &mut cur_width,
            &mut cur_ascent,
            &mut cur_descent,
        );
    }
    if !cur.is_empty() {
        lines.push(assemble_visual_line(
            &cur,
            cur_ascent,
            cur_descent,
            y_offset,
            false,
            tab_width,
        ));
    }

    lines
}

/// Fit-time pen advance of `item` at pen position `pen`. Fixed for text and
/// space runs; pen-relative for a tab (jumps to the next tab stop) so the fit's
/// running width matches what [`assemble_visual_line`] later lays out.
#[inline]
pub(crate) fn fit_advance(item: &ShapedWord, pen: f32, tab_width: f32) -> f32 {
    if item.is_tab && tab_width > 0.0 {
        advance_to_tab_stop(pen, tab_width) - pen
    } else {
        item.advance_width_lpx
    }
}

/// Record `word` on the current line (logical order), growing the running width
/// by `advance` and seeding the line's ascent/descent from the first item
/// placed. Glyph placement is deferred to [`assemble_visual_line`] at flush.
fn commit_item<'a>(
    word: &'a ShapedWord,
    advance: f32,
    cur: &mut Vec<&'a ShapedWord>,
    cur_width: &mut f32,
    cur_ascent: &mut f32,
    cur_descent: &mut f32,
) {
    if cur.is_empty() {
        *cur_ascent = word.ascent_lpx;
        *cur_descent = word.descent_lpx;
    }
    cur.push(word);
    *cur_width += advance;
}

/// Place a line's committed `items` (logical order) into a [`ShapedLine`] in
/// **visual** order.
///
/// Reorders items by UAX #9 rule L2 over their embedding levels and flows the
/// pen left-to-right, so RTL runs sit at the correct screen position while each
/// run's glyphs keep the visual order the native shaper returned. Populates
/// `runs` (level-runs in visual order) and `base_direction` (parity of the
/// minimum level = the paragraph base level).
///
/// When `rewrite_clusters_absolute` is set, each glyph's word-local `cluster`
/// is rewritten to a document-absolute byte offset (`source_byte_range.start +
/// local`) so the editable caret math stays byte-keyed.
///
/// **Pure-LTR / CJK fast path:** when every item is level 0 the logical order
/// *is* the visual order, so glyphs are placed exactly as the pre-bidi code did
/// and `runs` is left empty (implicit single LTR run) — byte-identical output.
///
/// `tab_width` sizes tab stops (`4 × space_advance` by convention); a tab item
/// advances the pen to the next stop measured from the line origin. Passing
/// `0.0` disables tab snapping (tabs fall back to their shaped advance). Tab
/// stops are computed against the running pen, which equals the line-origin
/// distance in LTR; in a reordered RTL line it is the logical-order pen, an
/// accepted approximation (tabs in RTL are rare and unverifiable on this host).
pub(crate) fn assemble_visual_line(
    items: &[&ShapedWord],
    ascent_lpx: f32,
    descent_lpx: f32,
    y_offset_lpx: f32,
    rewrite_clusters_absolute: bool,
    tab_width: f32,
) -> ShapedLine {
    // Place one item's glyphs at `pen_x`; return the pen advance. A tab has no
    // fixed advance — the pen jumps to the next stop and the tab's leading glyph
    // carries that whole advance so caret math (which sums `x_advance`) agrees.
    let place = |word: &ShapedWord, pen_x: f32, out: &mut Vec<ShapedGlyph>| -> f32 {
        let advance = if word.is_tab && tab_width > 0.0 {
            advance_to_tab_stop(pen_x, tab_width) - pen_x
        } else {
            word.advance_width_lpx
        };
        for (k, g) in word.glyphs.iter().enumerate() {
            let mut a = *g;
            a.position_lpx[0] += pen_x;
            if rewrite_clusters_absolute {
                a.cluster = (word.source_byte_range.start + g.cluster as usize) as u32;
            }
            if word.is_tab {
                a.x_advance_lpx = if k == 0 { advance } else { 0.0 };
            }
            out.push(a);
        }
        advance
    };

    // All-LTR (including the empty line): logical order = visual order, runs
    // empty. This reproduces the pre-bidi placement byte-for-byte.
    if items.iter().all(|w| w.level == 0) {
        let mut glyphs = Vec::new();
        let mut pen = 0.0f32;
        for w in items {
            pen += place(w, pen, &mut glyphs);
        }
        return ShapedLine {
            glyphs,
            width_lpx: pen,
            ascent_lpx,
            descent_lpx,
            y_offset_lpx,
            base_direction: Direction::Ltr,
            runs: Vec::new(),
        };
    }

    let levels: Vec<u8> = items.iter().map(|w| w.level).collect();
    let order = bidi::visual_order(&levels);
    let mut glyphs = Vec::new();
    let mut pen = 0.0f32;
    for &idx in &order {
        pen += place(items[idx], pen, &mut glyphs);
    }

    let base_level = levels.iter().copied().min().unwrap_or(0);
    ShapedLine {
        glyphs,
        width_lpx: pen,
        ascent_lpx,
        descent_lpx,
        y_offset_lpx,
        base_direction: dir_from_level(base_level),
        runs: build_visual_runs(items, &levels),
    }
}

/// Pen position after a horizontal tab: the next multiple of `tab_width` at or
/// beyond `pen + ε`. The ε ensures a tab on an exact stop advances to the *next*
/// stop (a tab is never zero-width). `tab_width` must be > 0 (callers pass
/// `4 × space_advance`).
#[inline]
pub(crate) fn advance_to_tab_stop(pen: f32, tab_width: f32) -> f32 {
    const EPSILON: f32 = 0.01;
    ((pen + EPSILON) / tab_width).ceil() * tab_width
}

/// Direction implied by an embedding level's parity.
#[inline]
pub(crate) fn dir_from_level(level: u8) -> Direction {
    if level.is_multiple_of(2) {
        Direction::Ltr
    } else {
        Direction::Rtl
    }
}

/// Coalesce a line's items into level-runs (adjacent same-level items merged,
/// byte ranges logical) and return them in **visual** order (rule L2).
fn build_visual_runs(items: &[&ShapedWord], levels: &[u8]) -> Vec<RunSpan> {
    // Consecutive same-level items share a contiguous logical byte range, so a
    // merged run's byte span is just [first.start, last.end]. The segmenter
    // emits items in logical order without interleaving levels; the assert locks
    // that invariant so a future reorder can't silently produce a gappy merge.
    let mut logical: Vec<(usize, usize, u8)> = Vec::new();
    for (w, &lvl) in items.iter().zip(levels) {
        let (s, e) = (w.source_byte_range.start, w.source_byte_range.end);
        match logical.last_mut() {
            Some(last) if last.2 == lvl => {
                debug_assert!(
                    s >= last.1,
                    "merged level-run items must be source-contiguous",
                );
                last.1 = e;
            }
            _ => logical.push((s, e, lvl)),
        }
    }
    let run_levels: Vec<u8> = logical.iter().map(|r| r.2).collect();
    bidi::visual_order(&run_levels)
        .into_iter()
        .map(|i| {
            let (s, e, lvl) = logical[i];
            RunSpan {
                byte_range: s..e,
                level: lvl,
                direction: dir_from_level(lvl),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_stop_arithmetic_at_boundaries() {
        let tw = 40.0; // 4 × 10px space
        // Pen at origin → first stop.
        assert_eq!(advance_to_tab_stop(0.0, tw), 40.0);
        // Mid-stop → next stop.
        assert_eq!(advance_to_tab_stop(15.0, tw), 40.0);
        // Exactly on a stop → advances to the *next* one (never zero-width).
        assert_eq!(advance_to_tab_stop(40.0, tw), 80.0);
        assert_eq!(advance_to_tab_stop(80.0, tw), 120.0);
    }
}
