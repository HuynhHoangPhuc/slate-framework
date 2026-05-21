//! Multi-line paragraph layout with greedy word wrap.
//!
//! Provides `greedy_wrap` for breaking text into wrapped lines,
//! `compute_alignment_offset` for paint-time alignment, and
//! `truncate_with_ellipsis` for single-line truncation.
//!
//! Wrapping is split into two stages so callers can re-fit to a new width
//! without re-shaping: [`shape_words`] shapes each whitespace-delimited word
//! exactly once (expensive, keyed on text+font), and [`wrap_shaped_words`]
//! fits those pre-shaped words to a width by pure arithmetic (cheap, keyed on
//! width). [`greedy_wrap`] is the convenience wrapper that does both.

use crate::backend::{Font, TextBackend};
use crate::bidi::{self, BidiRun};
use crate::error::TextError;
use crate::types::{Direction, RunSpan, ShapedGlyph, ShapedLine, TextAlignment};

/// A single whitespace-delimited word shaped in isolation.
///
/// `glyphs` carry word-origin-relative positions (`position_lpx[0]` starts at
/// 0); [`wrap_shaped_words`] shifts them by the running line pen when placing
/// the word. Produced by [`shape_words`], cached, and fit to any width with no
/// further shaping.
#[derive(Clone, Debug)]
pub struct ShapedWord {
    /// Glyphs with positions relative to the word origin (pen starts at 0).
    pub glyphs: Vec<ShapedGlyph>,
    /// Total advance width of the word in logical pixels.
    pub advance_width_lpx: f32,
    /// Ascent of the word (drives line ascent when first on a line).
    pub ascent_lpx: f32,
    /// Descent of the word (drives line descent when first on a line).
    pub descent_lpx: f32,
    /// UTF-8 byte span of the word in the original `text` passed to
    /// [`shape_words`]. Lets the multi-line wrap recover per-visual-line byte
    /// ranges (which `wrap_shaped_words` alone cannot, since it works on the
    /// pre-shaped glyph runs with no source pointer).
    pub source_byte_range: std::ops::Range<usize>,
    /// `true` when this item is a run of ASCII spaces (U+0020) rather than a
    /// text word. A space run carries one glyph per space byte (each advancing
    /// `space_width`) so every space is independently caret-addressable. The
    /// wrap fit treats it as a soft-break candidate whose trailing copy is
    /// absorbed at a soft wrap but kept (visible) at a hard line end.
    pub is_space_run: bool,
    /// UAX #9 embedding level of the level-run this item belongs to (even = LTR,
    /// odd = RTL). `0` on the pure-LTR / CJK path. Line assembly reorders a
    /// line's items into visual order from these levels (rule L2).
    pub level: u8,
    /// `true` when this item is a single horizontal tab (U+0009). A tab has no
    /// fixed advance: line assembly advances the pen to the next tab stop
    /// (`ceil((pen+ε)/tab_width)·tab_width`), so `advance_width_lpx` is ignored
    /// for tabs. Each tab is its own caret-addressable item.
    pub is_tab: bool,
    /// `true` when a UAX #14 soft line break may fall immediately before this
    /// item. The wrap fit may only start a new line at an item with this set
    /// (plus the always-allowed first item). ASCII words after a space, CJK
    /// ideographs, and the piece after a hyphen all carry it; the first piece of
    /// a run with no break opportunity before it does not.
    pub break_before: bool,
}

/// Shape every whitespace-delimited word in `text` exactly once.
///
/// Returns the ordered items (text words interleaved with ASCII-space runs)
/// plus the shared single-space advance (shaped once, for callers that still
/// want it). Pair with [`wrap_shaped_words`] to fit the items to any width with
/// zero further shaping calls — so re-wrap on a resize is pure arithmetic.
///
/// Every ASCII space (U+0020) is preserved as its own glyph (see
/// [`shape_words_in`]); empty input yields an empty list, but a whitespace-only
/// string yields a single space-run item.
pub fn shape_words<B: TextBackend>(
    backend: &B,
    font: &B::Font,
    text: &str,
) -> Result<(Vec<ShapedWord>, f32), TextError> {
    // Shape a space once to get the inter-word advance (reused at every join).
    let space_width = backend
        .shape_line(font, " ")
        .map(|s| s.width_lpx)
        .unwrap_or(0.0);

    let words = shape_words_in(backend, font, text, 0)?;
    Ok((words, space_width))
}

/// A segmentation unit handed to the native shaper: a contiguous span of the
/// source text that is a single resolved direction (and, in later phases, a
/// single break-bounded level-run).
///
/// Today the only segmenter is [`WhitespaceSegmenter`], which produces maximal
/// ASCII-space / non-space runs, all `Ltr` at level 0 — reproducing the
/// historical `shape_words_in` byte scan exactly. The bidi segmenter replaces
/// it with real level-runs once direction resolution is wired in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Segment {
    /// Byte range within the text passed to [`LineSegmenter::segments`].
    pub byte_range: std::ops::Range<usize>,
    /// Resolved direction of this span.
    pub direction: Direction,
    /// UAX #9 embedding level (0 for the whitespace segmenter).
    pub level: u8,
    /// `true` when this span is a single horizontal tab (U+0009).
    pub is_tab: bool,
    /// `true` when a UAX #14 break opportunity falls before this span's start.
    pub break_before: bool,
}

/// Splits a line of text into shaping segments.
///
/// The seam that lets later phases swap whitespace-word segmentation for bidi
/// level-run segmentation without touching the shaping/fit plumbing. The unit
/// of segmentation is whatever the native shaper should receive in one call.
pub(crate) trait LineSegmenter {
    /// Segment `text` (a single `\n`-free run) into ordered shaping spans.
    fn segments(&self, text: &str) -> Vec<Segment>;
}

/// Whitespace oracle segmenter: maximal runs of ASCII spaces (U+0020)
/// interleaved with non-space spans, all `Ltr` at level 0.
///
/// Byte-for-byte equivalent to the historical `shape_words_in` scan. The bidi
/// segmenter superseded it on the production path; it survives as the oracle
/// for the LTR-identity regression gate (the bidi segmenter must reproduce its
/// spans on pure-LTR / CJK text), hence test-only.
#[cfg(test)]
pub(crate) struct WhitespaceSegmenter;

#[cfg(test)]
impl LineSegmenter for WhitespaceSegmenter {
    fn segments(&self, text: &str) -> Vec<Segment> {
        let bytes = text.as_bytes();
        let mut segs = Vec::new();
        let mut i = 0usize;
        while i < bytes.len() {
            let start = i;
            let is_space = bytes[i] == b' ';
            // Extend the run while its space-ness matches.
            while i < bytes.len() && (bytes[i] == b' ') == is_space {
                i += 1;
            }
            segs.push(Segment {
                byte_range: start..i,
                direction: Direction::Ltr,
                level: 0,
                is_tab: false,
                break_before: false,
            });
        }
        segs
    }
}

/// Production segmenter: resolves UAX #9 bidi levels paragraph-wide and computes
/// UAX #14 break opportunities over the whole line, then emits one shaping span
/// per (level-run × break-opportunity × kind) cell. Kinds are text, ASCII-space
/// run, and single tab. Each span carries its run's direction and level, a
/// `break_before` flag (a soft break may start a line here), and `is_tab`. Line
/// assembly reorders the spans into visual order.
///
/// For pure-LTR text whose break opportunities coincide with spaces (all of
/// ASCII-Latin), this yields the same byte ranges as [`WhitespaceSegmenter`], so
/// the shaping output is unchanged. CJK splits at every ideograph (so it can
/// finally wrap), and a hyphen splits the word after it — both intended changes.
pub(crate) struct BidiSegmenter {
    /// Forced paragraph base direction; `None` auto-detects (first strong char).
    pub base: Option<Direction>,
}

impl LineSegmenter for BidiSegmenter {
    fn segments(&self, text: &str) -> Vec<Segment> {
        let resolved = bidi::resolve_line(text, self.base);
        // Break opportunities are a line-global partition orthogonal to bidi
        // level-runs; the shaping unit is their intersection. Computed once over
        // the whole line so offsets are comparable across runs.
        let breaks = crate::linebreak::break_offsets(text);
        let mut segs = Vec::new();
        for run in &resolved.logical_runs {
            split_run(text, run, &breaks, &mut segs);
        }
        segs
    }
}

/// Split one level-run into shaping spans, tagging each with the run's direction
/// and level. Boundaries fall at: kind changes (text / ASCII-space / tab), each
/// individual tab (own caret-addressable stop), and interior UAX #14 break
/// offsets inside a text span. Appends to `out` in logical (source) order.
fn split_run(text: &str, run: &BidiRun, breaks: &[usize], out: &mut Vec<Segment>) {
    let bytes = text.as_bytes();
    let end = run.byte_range.end;
    let mut i = run.byte_range.start;
    while i < end {
        let start = i;
        let is_tab = bytes[i] == b'\t';
        if is_tab {
            // Tab: its own single-byte span (each advances to its own stop).
            i += 1;
        } else if bytes[i] == b' ' {
            // Maximal ASCII-space run — the absorption / soft-break unit.
            while i < end && bytes[i] == b' ' {
                i += 1;
            }
        } else {
            // Text: extend over chars until the next space, tab, or interior
            // break offset (so CJK splits per ideograph, "foo-bar" after '-').
            loop {
                let ch = text[i..end].chars().next().expect("i < end is on a char");
                i += ch.len_utf8();
                if i >= end
                    || bytes[i] == b' '
                    || bytes[i] == b'\t'
                    || crate::linebreak::is_break_before(breaks, i)
                {
                    break;
                }
            }
        }
        out.push(Segment {
            byte_range: start..i,
            direction: run.direction,
            level: run.level,
            is_tab,
            break_before: crate::linebreak::is_break_before(breaks, start),
        });
    }
}

/// Shape `segment` into an ordered run of items, recording each item's byte
/// span as an absolute offset into the larger document (`segment_start` is the
/// byte offset of `segment` within that document; pass 0 when `segment` is the
/// whole text).
///
/// Drives off [`BidiSegmenter`] via the [`LineSegmenter`] seam: each emitted
/// span is shaped once via `shape_segment`, and its glyphs are tagged with the
/// span direction. A span whose first byte is U+0020 is a space run (one glyph
/// per space byte, caret-addressable, soft-break candidate); a single U+0009 is
/// a tab (caret-addressable, pen-relative advance at assembly); other spans are
/// text words. Each item carries the segment's `break_before` (whether a soft
/// line break may start here) and `is_tab` flags. Other Unicode whitespace
/// (NBSP, …) stays inside the surrounding text word and shapes as part of it.
///
/// Shared by [`shape_words`] (single segment, offset 0) and the multi-line
/// paragraph shaper, which calls it once per `\n`-delimited paragraph so item
/// ranges stay absolute across the document.
pub(crate) fn shape_words_in<B: TextBackend>(
    backend: &B,
    font: &B::Font,
    segment: &str,
    segment_start: usize,
) -> Result<Vec<ShapedWord>, TextError> {
    let segmenter = BidiSegmenter { base: None };
    let mut items = Vec::new();
    for seg in segmenter.segments(segment) {
        let slice = &segment[seg.byte_range.clone()];
        // Homogeneous spans (segmenter never mixes kinds), so the first byte
        // determines the whole run's space-ness. Tabs are a separate kind.
        let is_space = !seg.is_tab && slice.as_bytes().first() == Some(&b' ');
        let mut shaped = backend.shape_segment(font, slice, seg.direction)?;
        for g in &mut shaped.glyphs {
            g.direction = seg.direction;
        }
        items.push(ShapedWord {
            glyphs: shaped.glyphs,
            advance_width_lpx: shaped.width_lpx,
            ascent_lpx: shaped.ascent_lpx,
            descent_lpx: shaped.descent_lpx,
            source_byte_range: segment_start + seg.byte_range.start
                ..segment_start + seg.byte_range.end,
            is_space_run: is_space,
            level: seg.level,
            is_tab: seg.is_tab,
            break_before: seg.break_before,
        });
    }
    Ok(items)
}

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
            commit_item(sp, sp.advance_width_lpx, &mut cur, &mut cur_width, &mut cur_ascent, &mut cur_descent);
        }

        let placed_w = fit_advance(item, cur_width, tab_width);
        commit_item(item, placed_w, &mut cur, &mut cur_width, &mut cur_ascent, &mut cur_descent);
    }

    // Trailing/standalone spaces at the end of the text stay visible.
    if let Some(sp) = pending.take() {
        commit_item(sp, sp.advance_width_lpx, &mut cur, &mut cur_width, &mut cur_ascent, &mut cur_descent);
    }
    if !cur.is_empty() {
        lines.push(assemble_visual_line(
            &cur, cur_ascent, cur_descent, y_offset, false, tab_width,
        ));
    }

    lines
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

/// Wrap text into multiple lines using greedy first-fit algorithm.
///
/// Breaks at UAX #14 opportunities (after spaces, between CJK ideographs, after
/// hyphens, …), so space-less scripts wrap. Each returned `ShapedLine` has its
/// `y_offset_lpx` set to the vertical position relative to the paragraph origin.
///
/// # Behavior
///
/// - Words with no interior break opportunity wider than `max_width_lpx` are
///   placed on their own line without character-level breaking (may overflow
///   visually).
/// - Every ASCII space is preserved (one glyph per space byte); a run is
///   absorbed at a soft wrap but kept visible at the end of the text.
/// - Tabs (U+0009) advance the pen to the next tab stop (`4 × space_advance`).
/// - Empty input returns an empty `Vec`.
///
/// # Performance
///
/// Each word is shaped exactly once. Accumulated glyphs are joined with a
/// space-advance offset rather than re-shaping the full line string. As a
/// result cross-word kerning pairs that span the space boundary are not
/// captured — this is an acceptable tradeoff for O(W) vs O(W+L) shaping
/// calls (W = words, L = lines).
///
/// # Arguments
///
/// * `backend` - Text backend for shaping
/// * `font` - Font to use
/// * `text` - Input text to wrap
/// * `max_width_lpx` - Maximum line width in logical pixels
pub fn greedy_wrap<B: TextBackend>(
    backend: &B,
    font: &B::Font,
    text: &str,
    max_width_lpx: f32,
) -> Result<Vec<ShapedLine>, TextError> {
    // Handle empty text
    if text.is_empty() {
        return Ok(vec![]);
    }

    let metrics = font.metrics();
    let line_height = metrics.ascent_lpx - metrics.descent_lpx + metrics.line_gap_lpx;

    // Shape each word once, then fit by arithmetic. Splitting the two stages
    // lets the Text element cache the shaped words and re-fit on resize without
    // re-shaping; here we just chain them for the one-shot convenience path.
    let (words, space_width) = shape_words(backend, font, text)?;
    Ok(wrap_shaped_words(&words, space_width, line_height, max_width_lpx))
}

/// Compute horizontal offset for text alignment.
///
/// Returns the X offset to apply at paint time. Does not re-shape.
#[inline]
pub fn compute_alignment_offset(
    line_width_lpx: f32,
    container_width_lpx: f32,
    alignment: TextAlignment,
) -> f32 {
    match alignment {
        TextAlignment::Left => 0.0,
        TextAlignment::Center => (container_width_lpx - line_width_lpx) / 2.0,
        TextAlignment::Right => container_width_lpx - line_width_lpx,
    }
}

/// Truncate a shaped line with ellipsis if it exceeds max width.
///
/// Uses cumulative advance to find the optimal cut point, then appends "..."
/// glyphs shifted to sit immediately after the truncated text. Returns the
/// original line unchanged if it fits.
pub fn truncate_with_ellipsis<B: TextBackend>(
    backend: &B,
    font: &B::Font,
    shaped: &ShapedLine,
    max_width_lpx: f32,
) -> Result<ShapedLine, TextError> {
    // Already fits
    if shaped.width_lpx <= max_width_lpx {
        return Ok(shaped.clone());
    }

    // Shape ellipsis
    let ellipsis = backend.shape_line(font, "...")?;
    let target_width = max_width_lpx - ellipsis.width_lpx;

    // If ellipsis alone doesn't fit, return just ellipsis
    if target_width <= 0.0 {
        return Ok(ShapedLine {
            glyphs: ellipsis.glyphs,
            width_lpx: ellipsis.width_lpx,
            ascent_lpx: shaped.ascent_lpx,
            descent_lpx: shaped.descent_lpx,
            y_offset_lpx: shaped.y_offset_lpx,
            base_direction: shaped.base_direction,
            // Truncation invalidates source byte ranges; ellipsis display is
            // LTR-only, so drop runs (empty = implicit LTR).
            runs: Vec::new(),
        });
    }

    // Find cut point using cumulative width
    let mut cumulative = 0.0f32;
    let mut cut_idx = 0;
    for (i, g) in shaped.glyphs.iter().enumerate() {
        if cumulative + g.x_advance_lpx > target_width {
            break;
        }
        cumulative += g.x_advance_lpx;
        cut_idx = i + 1;
    }

    // Build truncated glyphs + ellipsis.
    //
    // Ellipsis was shaped in isolation, so its glyphs' `position_lpx[0]` are
    // [0..ellipsis.width_lpx). Shift them by `truncated_width` so they sit
    // immediately after the last truncated glyph (in the absolute-position
    // coordinate space the renderer expects).
    let mut truncated_glyphs = shaped.glyphs[..cut_idx].to_vec();
    let truncated_width = cumulative;

    for eg in &ellipsis.glyphs {
        let mut shifted = *eg;
        shifted.position_lpx[0] += truncated_width;
        truncated_glyphs.push(shifted);
    }

    Ok(ShapedLine {
        glyphs: truncated_glyphs,
        width_lpx: truncated_width + ellipsis.width_lpx,
        ascent_lpx: shaped.ascent_lpx,
        descent_lpx: shaped.descent_lpx,
        y_offset_lpx: shaped.y_offset_lpx,
        base_direction: shaped.base_direction,
        runs: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Project a segment to the fields the LTR-identity gate cares about: byte
    /// range, direction, level, tab-ness. `break_before` is new UAX #14 info the
    /// whitespace oracle does not model, so it is excluded from the comparison.
    fn seg_shape(s: &Segment) -> (std::ops::Range<usize>, Direction, u8, bool) {
        (s.byte_range.clone(), s.direction, s.level, s.is_tab)
    }

    /// LTR-identity gate: on ASCII-Latin text (UAX #14 breaks coincide with
    /// spaces) the bidi segmenter must emit the same span *boundaries* as the
    /// whitespace segmenter, so the shaping output is unchanged. CJK is excluded
    /// here — it now splits per ideograph (its own test below).
    #[test]
    fn bidi_segmenter_matches_whitespace_segmenter_on_ascii() {
        let bidi = BidiSegmenter { base: None };
        let ws = WhitespaceSegmenter;
        for text in ["", "hello", "ab cd", "a  b", " ab ", "the quick brown fox"] {
            let b: Vec<_> = bidi.segments(text).iter().map(seg_shape).collect();
            let w: Vec<_> = ws.segments(text).iter().map(seg_shape).collect();
            assert_eq!(b, w, "segmenters diverged on {text:?}");
        }
    }

    /// CJK has no spaces: the bidi segmenter must now split it at every UAX #14
    /// break (between ideographs) so it can wrap. Each non-first piece carries
    /// `break_before`.
    #[test]
    fn bidi_segmenter_splits_cjk_at_break_opportunities() {
        let segs = BidiSegmenter { base: None }.segments("日本語");
        let ranges: Vec<_> = segs.iter().map(|s| s.byte_range.clone()).collect();
        assert_eq!(ranges, vec![0..3, 3..6, 6..9]);
        assert!(!segs[0].break_before, "first piece has no break before it");
        assert!(segs[1].break_before && segs[2].break_before);
    }

    /// A break opportunity after a hyphen splits the word; the second piece is a
    /// soft-break start.
    #[test]
    fn bidi_segmenter_splits_after_hyphen() {
        let segs = BidiSegmenter { base: None }.segments("foo-bar");
        let ranges: Vec<_> = segs.iter().map(|s| s.byte_range.clone()).collect();
        assert_eq!(ranges, vec![0..4, 4..7]);
        assert!(segs[1].break_before);
    }

    /// Each tab is its own segment, flagged `is_tab`, never merged with text or
    /// with an adjacent tab.
    #[test]
    fn tab_is_an_isolated_segment() {
        let segs = BidiSegmenter { base: None }.segments("a\t\tb");
        let shapes: Vec<_> = segs
            .iter()
            .map(|s| (s.byte_range.clone(), s.is_tab))
            .collect();
        assert_eq!(
            shapes,
            vec![(0..1, false), (1..2, true), (2..3, true), (3..4, false)]
        );
    }

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

    /// On RTL / mixed text the bidi segmenter must tag runs with the resolved
    /// direction (the whitespace segmenter cannot).
    #[test]
    fn bidi_segmenter_tags_rtl_runs() {
        let segs = BidiSegmenter { base: None }.segments("abc אבג");
        // "abc" + " " are LTR; "אבג" is RTL.
        assert!(segs.iter().any(|s| s.direction == Direction::Rtl));
        assert!(segs.iter().any(|s| s.direction == Direction::Ltr));
        // Byte coverage is gap-free and ordered.
        assert_eq!(segs.first().unwrap().byte_range.start, 0);
        assert_eq!(segs.last().unwrap().byte_range.end, "abc אבג".len());
    }

    #[test]
    fn alignment_offsets() {
        assert_eq!(
            compute_alignment_offset(100.0, 200.0, TextAlignment::Left),
            0.0
        );
        assert_eq!(
            compute_alignment_offset(100.0, 200.0, TextAlignment::Center),
            50.0
        );
        assert_eq!(
            compute_alignment_offset(100.0, 200.0, TextAlignment::Right),
            100.0
        );
    }

    #[test]
    fn alignment_wider_than_container() {
        // Line wider than container: negative offset for right/center
        assert_eq!(
            compute_alignment_offset(200.0, 100.0, TextAlignment::Left),
            0.0
        );
        assert_eq!(
            compute_alignment_offset(200.0, 100.0, TextAlignment::Center),
            -50.0
        );
        assert_eq!(
            compute_alignment_offset(200.0, 100.0, TextAlignment::Right),
            -100.0
        );
    }
}
