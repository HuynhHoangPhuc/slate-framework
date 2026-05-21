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
use crate::error::TextError;
use crate::types::{Direction, ShapedGlyph, ShapedLine, TextAlignment};

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

/// Phase-1 segmenter: maximal runs of ASCII spaces (U+0020) interleaved with
/// non-space spans, all `Ltr` at level 0.
///
/// Byte-for-byte equivalent to the historical `shape_words_in` scan, so the
/// pure-LTR / CJK shaping output is unchanged. Retained after the bidi
/// segmenter lands as the fixture for the LTR-identity regression gate.
pub(crate) struct WhitespaceSegmenter;

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
            });
        }
        segs
    }
}

/// Shape `segment` into an ordered run of items, recording each item's byte
/// span as an absolute offset into the larger document (`segment_start` is the
/// byte offset of `segment` within that document; pass 0 when `segment` is the
/// whole text).
///
/// Drives off [`WhitespaceSegmenter`] via the [`LineSegmenter`] seam: each
/// emitted span is shaped once via `shape_line`, and its glyphs are tagged with
/// the span direction. A span whose first byte is U+0020 is a space run (one
/// glyph per space byte, caret-addressable, soft-break candidate); other spans
/// are text words. Non-space whitespace (`\t`, NBSP, …) stays inside the
/// surrounding text word and shapes as part of it.
///
/// Shared by [`shape_words`] (single segment, offset 0) and the multi-line
/// paragraph shaper, which calls it once per `\n`-delimited paragraph so item
/// ranges stay absolute across the document. With `WhitespaceSegmenter` the
/// emitted item list is byte-identical to the historical scan.
pub(crate) fn shape_words_in<B: TextBackend>(
    backend: &B,
    font: &B::Font,
    segment: &str,
    segment_start: usize,
) -> Result<Vec<ShapedWord>, TextError> {
    let segmenter = WhitespaceSegmenter;
    let mut items = Vec::new();
    for seg in segmenter.segments(segment) {
        let slice = &segment[seg.byte_range.clone()];
        // Homogeneous spans (segmenter never mixes space/non-space), so the
        // first byte determines the whole run's space-ness.
        let is_space = slice.as_bytes().first() == Some(&b' ');
        let mut shaped = backend.shape_line(font, slice)?;
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
pub fn wrap_shaped_words(
    items: &[ShapedWord],
    space_width: f32,
    line_height: f32,
    max_width_lpx: f32,
) -> Vec<ShapedLine> {
    let _ = space_width; // spaces are explicit items now; kept for API stability
    let mut lines: Vec<ShapedLine> = Vec::new();
    let mut cur: Vec<ShapedGlyph> = Vec::new();
    let mut cur_width = 0.0f32;
    let mut cur_ascent = 0.0f32;
    let mut cur_descent = 0.0f32;
    let mut y_offset = 0.0f32;
    // A trailing space run not yet committed to the line's visible width.
    let mut pending: Option<&ShapedWord> = None;

    // Append `word`'s glyphs at the current pen and grow the line width.
    let commit = |word: &ShapedWord,
                  cur: &mut Vec<ShapedGlyph>,
                  cur_width: &mut f32,
                  cur_ascent: &mut f32,
                  cur_descent: &mut f32| {
        if cur.is_empty() {
            *cur_ascent = word.ascent_lpx;
            *cur_descent = word.descent_lpx;
        }
        let pen_x = *cur_width;
        for g in &word.glyphs {
            let mut adjusted = *g;
            adjusted.position_lpx[0] += pen_x;
            cur.push(adjusted);
        }
        *cur_width += word.advance_width_lpx;
    };

    for item in items {
        if item.is_space_run {
            // Hold the run: it is either committed before a following word or
            // absorbed at a wrap. (Scanner never emits two runs in a row.)
            pending = Some(item);
            continue;
        }

        let pending_w = pending.map(|s| s.advance_width_lpx).unwrap_or(0.0);
        let candidate = if cur.is_empty() {
            item.advance_width_lpx
        } else {
            cur_width + pending_w + item.advance_width_lpx
        };

        if candidate > max_width_lpx && !cur.is_empty() {
            // Wrap before this word; absorb the pending space run.
            lines.push(build_line_from_glyphs(
                std::mem::take(&mut cur),
                cur_width,
                cur_ascent,
                cur_descent,
                y_offset,
            ));
            y_offset += line_height;
            cur_width = 0.0;
            pending = None;
        } else if let Some(sp) = pending.take() {
            // Word fits with the run: make the spaces visible, then the word.
            commit(sp, &mut cur, &mut cur_width, &mut cur_ascent, &mut cur_descent);
        }

        commit(item, &mut cur, &mut cur_width, &mut cur_ascent, &mut cur_descent);
    }

    // Trailing/standalone spaces at the end of the text stay visible.
    if let Some(sp) = pending.take() {
        commit(sp, &mut cur, &mut cur_width, &mut cur_ascent, &mut cur_descent);
    }
    if !cur.is_empty() {
        lines.push(build_line_from_glyphs(
            cur, cur_width, cur_ascent, cur_descent, y_offset,
        ));
    }

    lines
}

/// Wrap text into multiple lines using greedy first-fit algorithm.
///
/// Breaks on spaces only (UAX #14 deferred to Phase 2c). Each returned
/// `ShapedLine` has its `y_offset_lpx` set to the vertical position
/// relative to the paragraph origin.
///
/// # Behavior
///
/// - Words longer than `max_width_lpx` are placed on their own line without
///   character-level breaking (may overflow visually).
/// - Every ASCII space is preserved (one glyph per space byte); a run is
///   absorbed at a soft wrap but kept visible at the end of the text.
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

/// Build a `ShapedLine` from pre-accumulated glyphs.
///
/// Glyphs must already have their `position_lpx[0]` adjusted to their
/// absolute positions within the line. `width_lpx` should be the total
/// advance width (sum of per-glyph advances including inter-word space
/// advances).
fn build_line_from_glyphs(
    glyphs: Vec<ShapedGlyph>,
    width_lpx: f32,
    ascent_lpx: f32,
    descent_lpx: f32,
    y_offset_lpx: f32,
) -> ShapedLine {
    ShapedLine {
        glyphs,
        width_lpx,
        ascent_lpx,
        descent_lpx,
        y_offset_lpx,
        base_direction: Direction::Ltr,
        runs: Vec::new(),
    }
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
