//! Byte-aware multi-line wrap for editable multi-line text (`TextArea`).
//!
//! `wrap_shaped_words` fits pre-shaped words to a width but discards which
//! source bytes landed on which visual line — the multi-line caret model needs
//! that mapping. This module adds it:
//!
//! - [`shape_document`] splits `text` on hard `\n` into paragraphs, shapes each
//!   paragraph's words exactly once (reusing [`shape_words_in`]), and keeps the
//!   absolute byte coverage of each paragraph (the trailing `\n` is folded into
//!   the preceding paragraph so coverage is gap-free).
//! - [`wrap_document`] fits that shaped document to a width by pure arithmetic
//!   (no shaping — re-fit on resize is free), producing [`VisualLine`]s that
//!   each carry an absolute `byte_start..byte_end`. Across all lines these
//!   ranges are contiguous and cover `0..text.len()`.
//!
//! # Conventions
//!
//! - **line_height**: `ascent - descent + line_gap` (paragraph.rs definition,
//!   includes `line_gap` for correct inter-line spacing). Uniform per document
//!   since `TextArea` uses a single font.
//! - **Caret affinity**: a byte that is both the end of line N and the start of
//!   line N+1 (a soft wrap boundary) belongs to line N+1's range, so the caret
//!   resolves to the next line's head. The newline byte of a hard break is
//!   folded into the preceding line. No affinity bit is stored.
//! - **Over-wide word**: a word wider than `max_width` is broken at grapheme
//!   (cluster) boundaries so no glyph run exceeds the box (no clip this round).

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::backend::{Font, TextBackend};
use crate::error::TextError;
use crate::paragraph::{ShapedWord, shape_words_in};
use crate::types::{ShapedGlyph, ShapedLine};

/// One `\n`-delimited paragraph's pre-shaped words plus its absolute byte
/// coverage. `byte_range` includes the paragraph's trailing `\n` (except the
/// last paragraph, which ends at `text.len()`), so concatenated paragraph
/// ranges tile `0..text.len()` with no gaps.
#[derive(Clone, Debug)]
pub struct ShapedParagraph {
    /// Whitespace-delimited words, byte ranges absolute into the document.
    pub words: Vec<ShapedWord>,
    /// Absolute byte coverage of this paragraph (incl. trailing `\n`).
    pub byte_range: Range<usize>,
}

/// A whole document shaped once: paragraphs + shared metrics. Fit to any width
/// with [`wrap_document`] at zero further shaping cost.
#[derive(Clone, Debug)]
pub struct ShapedDocument {
    /// Paragraphs in document order.
    pub paragraphs: Vec<ShapedParagraph>,
    /// Shared inter-word space advance (shaped once).
    pub space_width: f32,
    /// Ascent above baseline (lpx, positive) — uniform per document.
    pub ascent_lpx: f32,
    /// Descent below baseline (lpx, negative) — uniform per document.
    pub descent_lpx: f32,
    /// Vertical advance between visual lines (lpx).
    pub line_height_lpx: f32,
}

/// One wrapped visual line: its shaped glyphs (positioned from x = 0, with
/// `y_offset_lpx` set to the line's vertical position) plus the absolute byte
/// range it covers.
#[derive(Clone, Debug)]
pub struct VisualLine {
    /// Glyphs + metrics for this line. `y_offset_lpx` is the line's top.
    pub line: ShapedLine,
    /// Absolute byte offset of the first byte on this line.
    pub byte_start: usize,
    /// Absolute byte offset one past the last byte on this line. Equals the
    /// next line's `byte_start` (or `text.len()` for the final line).
    pub byte_end: usize,
}

/// The fitted multi-line layout: visual lines top-to-bottom + the total height.
#[derive(Clone, Debug)]
pub struct MultilineLayout {
    /// Visual lines in top-to-bottom order. Always ≥ 1 line (empty text → one
    /// empty line) so the caret always has a line to sit on.
    pub lines: Vec<VisualLine>,
    /// Sum of all line heights (lpx) = `lines.len() * line_height_lpx`.
    pub total_height_lpx: f32,
    /// Vertical advance between lines (lpx).
    pub line_height_lpx: f32,
}

/// Shape `text` into a [`ShapedDocument`]: split on hard `\n`, shape each
/// paragraph's words once. Pair with [`wrap_document`] to fit to a width.
pub fn shape_document<B: TextBackend>(
    backend: &B,
    font: &B::Font,
    text: &str,
) -> Result<ShapedDocument, TextError> {
    let space_width = backend
        .shape_line(font, " ")
        .map(|s| s.width_lpx)
        .unwrap_or(0.0);
    let metrics = font.metrics();
    let line_height_lpx = metrics.ascent_lpx - metrics.descent_lpx + metrics.line_gap_lpx;

    // Split on '\n', tracking each paragraph's absolute start. Coverage of a
    // paragraph runs to the *next* paragraph's start (folding in the '\n'), so
    // ranges are gap-free; the final paragraph ends at text.len().
    let mut spans: Vec<(usize, &str)> = Vec::new();
    let mut offset = 0usize;
    for para in text.split('\n') {
        spans.push((offset, para));
        offset += para.len() + 1; // +1 for the consumed '\n'
    }

    let total_len = text.len();
    let mut paragraphs = Vec::with_capacity(spans.len());
    for (i, (start, para)) in spans.iter().enumerate() {
        let coverage_end = spans.get(i + 1).map(|(s, _)| *s).unwrap_or(total_len);
        let words = shape_words_in(backend, font, para, *start)?;
        paragraphs.push(ShapedParagraph {
            words,
            byte_range: *start..coverage_end,
        });
    }

    Ok(ShapedDocument {
        paragraphs,
        space_width,
        ascent_lpx: metrics.ascent_lpx,
        descent_lpx: metrics.descent_lpx,
        line_height_lpx,
    })
}

/// Fit a [`ShapedDocument`] to `max_width_lpx` by pure arithmetic — no shaping.
///
/// Each paragraph yields ≥ 1 visual line (empty paragraph → one empty line).
/// Line byte ranges are made contiguous: the first line of a paragraph starts
/// at the paragraph's coverage start (absorbing any leading whitespace) and
/// each line ends where the next begins; the last line of the last paragraph
/// ends at `text.len()`.
pub fn wrap_document(doc: &ShapedDocument, max_width_lpx: f32) -> MultilineLayout {
    let line_height = doc.line_height_lpx;

    // First pass: collect (glyphs/width per line, byte_start) across the whole
    // document, so byte_end can be filled from the following line's start.
    let mut raw: Vec<(ShapedLine, usize)> = Vec::new();
    for para in &doc.paragraphs {
        fit_paragraph(doc, &para.words, max_width_lpx, &para.byte_range, &mut raw);
    }

    let total_len = doc
        .paragraphs
        .last()
        .map(|p| p.byte_range.end)
        .unwrap_or(0);

    let mut lines = Vec::with_capacity(raw.len());
    for i in 0..raw.len() {
        let byte_start = raw[i].1;
        let byte_end = raw.get(i + 1).map(|(_, s)| *s).unwrap_or(total_len);
        let mut line = std::mem::replace(&mut raw[i].0, empty_shaped_line(doc));
        line.y_offset_lpx = i as f32 * line_height;
        lines.push(VisualLine {
            line,
            byte_start,
            byte_end,
        });
    }

    let total_height_lpx = lines.len() as f32 * line_height;
    MultilineLayout {
        lines,
        total_height_lpx,
        line_height_lpx: line_height,
    }
}

/// Greedy first-fit of one paragraph's words, appending `(line, byte_start)`
/// pairs to `out`. The first emitted line's `byte_start` is forced to the
/// paragraph's coverage start so leading whitespace and the contiguity chain
/// stay intact. Always emits ≥ 1 line.
fn fit_paragraph(
    doc: &ShapedDocument,
    words: &[ShapedWord],
    max_width: f32,
    coverage: &Range<usize>,
    out: &mut Vec<(ShapedLine, usize)>,
) {
    let first_idx = out.len();
    let mut cur: Vec<ShapedGlyph> = Vec::new();
    let mut cur_width = 0.0f32;
    let mut cur_start = 0usize;
    // A trailing space run held back: committed (visible) before a following
    // word that fits, or absorbed at a soft wrap / before an over-wide word.
    let mut pending: Option<&ShapedWord> = None;

    // Append an item's glyphs at the running pen, rewriting word-local clusters
    // to document-absolute byte offsets so caret/hit-test math is byte-keyed
    // end to end (applies to space-run glyphs identically).
    fn place(word: &ShapedWord, cur: &mut Vec<ShapedGlyph>, cur_width: &mut f32) {
        let pen_x = *cur_width;
        for g in &word.glyphs {
            let mut adjusted = *g;
            adjusted.position_lpx[0] += pen_x;
            adjusted.cluster = (word.source_byte_range.start + g.cluster as usize) as u32;
            cur.push(adjusted);
        }
        *cur_width += word.advance_width_lpx;
    }

    for word in words {
        if word.is_space_run {
            // Hold the run; the scanner never emits two runs back to back.
            pending = Some(word);
            continue;
        }

        // A word that cannot fit even on its own line is broken at grapheme
        // boundaries. The pending run is a soft break → absorb it, flush the
        // in-progress line, then emit the pieces.
        if word.advance_width_lpx > max_width {
            pending = None;
            if !cur.is_empty() {
                out.push((build_shaped_line(doc, std::mem::take(&mut cur), cur_width), cur_start));
                cur_width = 0.0;
            }
            for (glyphs, width, start) in break_word(word, max_width) {
                out.push((build_shaped_line(doc, glyphs, width), start));
            }
            continue;
        }

        let pending_w = pending.map(|s| s.advance_width_lpx).unwrap_or(0.0);
        let width_with = if cur.is_empty() {
            word.advance_width_lpx
        } else {
            cur_width + pending_w + word.advance_width_lpx
        };
        if width_with > max_width && !cur.is_empty() {
            // Wrap before this word; absorb the pending run (no visible width
            // on either line) and start the next line with the word.
            out.push((build_shaped_line(doc, std::mem::take(&mut cur), cur_width), cur_start));
            cur_width = 0.0;
            pending = None;
            cur_start = word.source_byte_range.start;
            place(word, &mut cur, &mut cur_width);
        } else {
            if cur.is_empty() {
                // Line begins at the pending run's start (leading spaces stay
                // visible) or at the word if there is no pending run.
                cur_start = pending
                    .map(|s| s.source_byte_range.start)
                    .unwrap_or(word.source_byte_range.start);
            }
            if let Some(sp) = pending.take() {
                place(sp, &mut cur, &mut cur_width);
            }
            place(word, &mut cur, &mut cur_width);
        }
    }

    // Trailing/standalone spaces at the paragraph (hard line) end stay visible
    // and caret-addressable.
    if let Some(sp) = pending.take() {
        if cur.is_empty() {
            cur_start = sp.source_byte_range.start;
        }
        place(sp, &mut cur, &mut cur_width);
    }
    if !cur.is_empty() {
        out.push((build_shaped_line(doc, cur, cur_width), cur_start));
    }

    if out.len() == first_idx {
        // Empty paragraph: one zero-glyph line so the caret can sit on it.
        out.push((empty_shaped_line(doc), coverage.start));
    }

    // Force the paragraph's first line to start at the coverage boundary so the
    // contiguity chain has no gap for leading whitespace.
    out[first_idx].1 = coverage.start;
}

/// Break an over-wide word into sub-lines at cluster (grapheme) boundaries so
/// no piece exceeds `max_width`. Returns `(glyphs, width, byte_start)` per
/// piece, glyphs re-zeroed to start at pen 0. A single cluster wider than the
/// box is emitted alone (cannot break below one grapheme).
fn break_word(word: &ShapedWord, max_width: f32) -> Vec<(Vec<ShapedGlyph>, f32, usize)> {
    let word_start = word.source_byte_range.start;
    let mut out: Vec<(Vec<ShapedGlyph>, f32, usize)> = Vec::new();

    let mut sub: Vec<ShapedGlyph> = Vec::new();
    let mut sub_width = 0.0f32;
    let mut sub_origin_x = 0.0f32;
    let mut sub_cluster = 0u32;

    // Cluster boundaries: glyphs sharing a `cluster` value are one grapheme and
    // must not be split. Accumulate per-cluster, breaking between clusters.
    let mut i = 0usize;
    while i < word.glyphs.len() {
        let cluster = word.glyphs[i].cluster;
        let mut j = i;
        let mut cluster_width = 0.0f32;
        while j < word.glyphs.len() && word.glyphs[j].cluster == cluster {
            cluster_width += word.glyphs[j].x_advance_lpx;
            j += 1;
        }

        if !sub.is_empty() && sub_width + cluster_width > max_width {
            out.push(finish_sub(&mut sub, sub_origin_x, sub_width, word_start + sub_cluster as usize));
            sub_width = 0.0;
        }
        if sub.is_empty() {
            sub_origin_x = word.glyphs[i].position_lpx[0];
            sub_cluster = cluster;
        }
        for g in &word.glyphs[i..j] {
            let mut a = *g;
            // Document-absolute cluster (see fit_paragraph) for byte-keyed math.
            a.cluster = (word_start + g.cluster as usize) as u32;
            sub.push(a);
        }
        sub_width += cluster_width;
        i = j;
    }
    if !sub.is_empty() {
        out.push(finish_sub(&mut sub, sub_origin_x, sub_width, word_start + sub_cluster as usize));
    }
    out
}

/// Drain `sub` into an owned glyph vec re-zeroed by `origin_x` (so the piece's
/// first glyph sits at pen 0), returning `(glyphs, width, byte_start)`.
fn finish_sub(
    sub: &mut Vec<ShapedGlyph>,
    origin_x: f32,
    width: f32,
    byte_start: usize,
) -> (Vec<ShapedGlyph>, f32, usize) {
    let glyphs: Vec<ShapedGlyph> = sub
        .drain(..)
        .map(|mut g| {
            g.position_lpx[0] -= origin_x;
            g
        })
        .collect();
    (glyphs, width, byte_start)
}

/// Build a `ShapedLine` from positioned glyphs with the document's uniform
/// ascent/descent. `y_offset_lpx` is filled by [`wrap_document`].
fn build_shaped_line(doc: &ShapedDocument, glyphs: Vec<ShapedGlyph>, width_lpx: f32) -> ShapedLine {
    ShapedLine {
        glyphs,
        width_lpx,
        ascent_lpx: doc.ascent_lpx,
        descent_lpx: doc.descent_lpx,
        y_offset_lpx: 0.0,
        base_direction: crate::types::Direction::Ltr,
        runs: Vec::new(),
    }
}

/// A zero-glyph line carrying the document's metrics, for empty paragraphs.
fn empty_shaped_line(doc: &ShapedDocument) -> ShapedLine {
    build_shaped_line(doc, Vec::new(), 0.0)
}

impl MultilineLayout {
    /// Resolve an absolute caret byte offset to the index of the visual line it
    /// renders on.
    ///
    /// Applies the no-affinity rule: a byte that is both the end of line N and
    /// the start of line N+1 (a soft wrap boundary) resolves to line N+1, since
    /// that is the line whose `byte_start` equals the byte. The only byte that
    /// resolves to the final line's end is `text.len()` (document end), which
    /// has no following line. Returns 0 for an empty layout.
    pub fn line_for_byte(&self, byte: usize) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        for (i, line) in self.lines.iter().enumerate() {
            // `byte_start <= byte < byte_end` claims interior bytes and, via the
            // next line's `byte_start == prev.byte_end`, hands a boundary byte to
            // the later line.
            if byte < line.byte_end {
                return i;
            }
        }
        // byte >= last byte_end (document end or past it) → final line.
        self.lines.len() - 1
    }

    /// Resolve an absolute caret byte offset to its on-screen position:
    /// `(line_index, x_lpx, y_lpx)`. `x_lpx` is line-relative (pen-x); `y_lpx`
    /// is the line's top. Uses [`Self::line_for_byte`] for the affinity rule.
    /// Returns `(0, 0.0, 0.0)` for an empty layout.
    pub fn caret_position(&self, byte: usize) -> (usize, f32, f32) {
        if self.lines.is_empty() {
            return (0, 0.0, 0.0);
        }
        let idx = self.line_for_byte(byte);
        let vline = &self.lines[idx];
        // Pen-x = sum of advances of glyphs strictly before the caret byte
        // (clusters are document-absolute). This matches `pixel_x_at_byte` and,
        // unlike reading the next glyph's position, does not jump across an
        // inter-word space gap when the caret sits at a word's trailing edge.
        let mut pen = 0.0f32;
        let mut x = vline.line.width_lpx;
        for g in &vline.line.glyphs {
            if g.cluster as usize >= byte {
                x = pen;
                break;
            }
            pen += g.x_advance_lpx;
        }
        (idx, x, vline.line.y_offset_lpx)
    }

    /// Absolute byte one past the last *visible* glyph cluster on line `idx` —
    /// the caret-addressable end of that visual line. Excludes a trailing hard
    /// `\n` and the whitespace absorbed at a soft-wrap boundary, so End and
    /// vertical motion keep the caret on the line rather than slipping to the
    /// next line's head (which the no-affinity rule would otherwise do). An
    /// empty line resolves to its own `byte_start`.
    pub fn line_caret_end(&self, text: &str, idx: usize) -> usize {
        match self.lines.get(idx) {
            Some(vline) => match vline.line.glyphs.last() {
                Some(g) => next_grapheme(text, g.cluster as usize),
                None => vline.byte_start,
            },
            None => 0,
        }
    }

    /// Resolve a line-relative pixel-x on visual line `line_idx` to an absolute
    /// caret byte offset, clamped to `[byte_start, line_caret_end]` so vertical
    /// motion never lands the caret on a neighbouring line. `x_lpx` is measured
    /// from the line's left edge.
    ///
    /// Used by vertical caret navigation (↑/↓) to preserve the sticky column:
    /// the desired x is round-tripped against the target line's glyphs with the
    /// same midpoint rule the pointer hit-test (`byte_at_pixel_x`) uses.
    pub fn byte_at_line_x(&self, text: &str, line_idx: usize, x_lpx: f32) -> usize {
        let Some(vline) = self.lines.get(line_idx) else {
            return 0;
        };
        let start = vline.byte_start;
        let end = self.line_caret_end(text, line_idx);
        let width = vline.line.width_lpx;
        if x_lpx <= 0.0 {
            return start;
        }
        if x_lpx >= width {
            return end;
        }

        // Pen-x at the leading edge of each cluster (clusters are doc-absolute
        // and non-decreasing in LTR order), mirroring `byte_at_pixel_x`.
        let mut cluster_pen: Vec<(usize, f32)> = Vec::with_capacity(vline.line.glyphs.len());
        let mut pen = 0.0f32;
        for g in &vline.line.glyphs {
            let c = g.cluster as usize;
            match cluster_pen.last() {
                Some(&(last_c, _)) if last_c == c => {}
                _ => cluster_pen.push((c, pen)),
            }
            pen += g.x_advance_lpx;
        }
        let mut cursor = 0usize;
        let pen_at = |byte: usize, cursor: &mut usize| -> f32 {
            while *cursor < cluster_pen.len() && cluster_pen[*cursor].0 < byte {
                *cursor += 1;
            }
            cluster_pen.get(*cursor).map(|&(_, p)| p).unwrap_or(width)
        };

        // Walk grapheme boundaries strictly inside the line's addressable range,
        // applying the midpoint rule. Boundaries are absolute byte offsets.
        let mut last_b = start;
        let mut last_x = 0.0f32;
        for (rel, _) in text[start..end].grapheme_indices(true) {
            if rel == 0 {
                continue;
            }
            let b = start + rel;
            let x = pen_at(b, &mut cursor);
            if x_lpx < x {
                let mid = (last_x + x) * 0.5;
                return if x_lpx < mid { last_b } else { b };
            }
            last_b = b;
            last_x = x;
        }
        let mid = (last_x + width) * 0.5;
        if x_lpx < mid { last_b } else { end }
    }
}

/// Byte offset of the grapheme boundary immediately after `byte` in `text`.
/// `byte` is assumed to be a grapheme boundary (glyph clusters always are), so
/// this returns the end of the grapheme starting at `byte`. Clamped to
/// `text.len()`.
fn next_grapheme(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    let mut cursor = unicode_segmentation::GraphemeCursor::new(byte, text.len(), true);
    match cursor.next_boundary(text, 0) {
        Ok(Some(b)) => b,
        _ => text.len(),
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

    fn vline(glyphs: Vec<ShapedGlyph>, byte_start: usize, byte_end: usize, y: f32) -> VisualLine {
        let width: f32 = glyphs.iter().map(|g| g.x_advance_lpx).sum();
        VisualLine {
            line: ShapedLine {
                glyphs,
                width_lpx: width,
                ascent_lpx: 10.0,
                descent_lpx: -2.0,
                y_offset_lpx: y,
                base_direction: crate::types::Direction::Ltr,
                runs: Vec::new(),
            },
            byte_start,
            byte_end,
        }
    }

    /// "ab\ncd": line0 = "ab" (covers the trailing '\n'), line1 = "cd".
    fn two_line_layout() -> MultilineLayout {
        MultilineLayout {
            lines: vec![
                vline(vec![glyph(0, 5.0), glyph(1, 6.0)], 0, 3, 0.0),
                vline(vec![glyph(3, 7.0), glyph(4, 8.0)], 3, 5, 12.0),
            ],
            total_height_lpx: 24.0,
            line_height_lpx: 12.0,
        }
    }

    #[test]
    fn line_for_byte_applies_no_affinity_at_boundary() {
        let l = two_line_layout();
        // Byte 3 is line0.byte_end and line1.byte_start → resolves to line1.
        assert_eq!(l.line_for_byte(2), 0);
        assert_eq!(l.line_for_byte(3), 1);
        // Document end resolves to the final line.
        assert_eq!(l.line_for_byte(5), 1);
        assert_eq!(l.line_for_byte(999), 1);
    }

    #[test]
    fn line_caret_end_excludes_trailing_newline() {
        let text = "ab\ncd";
        let l = two_line_layout();
        // After 'b' (byte 2), before the '\n' — caret stays on line0.
        assert_eq!(l.line_caret_end(text, 0), 2);
        // After 'd' = document end.
        assert_eq!(l.line_caret_end(text, 1), 5);
    }

    #[test]
    fn line_caret_end_empty_line_is_byte_start() {
        let text = "a\n\nb"; // line1 is the empty paragraph.
        let l = MultilineLayout {
            lines: vec![
                vline(vec![glyph(0, 5.0)], 0, 2, 0.0),
                vline(vec![], 2, 3, 12.0),
                vline(vec![glyph(3, 6.0)], 3, 4, 24.0),
            ],
            total_height_lpx: 36.0,
            line_height_lpx: 12.0,
        };
        assert_eq!(l.line_caret_end(text, 1), 2);
    }

    #[test]
    fn byte_at_line_x_clamps_to_line_range() {
        let text = "ab\ncd";
        let l = two_line_layout();
        // Left of the line → byte_start.
        assert_eq!(l.byte_at_line_x(text, 0, -10.0), 0);
        assert_eq!(l.byte_at_line_x(text, 1, 0.0), 3);
        // Right of the line → caret-addressable end (not byte_end with '\n').
        assert_eq!(l.byte_at_line_x(text, 0, 1000.0), 2);
        assert_eq!(l.byte_at_line_x(text, 1, 1000.0), 5);
    }

    #[test]
    fn byte_at_line_x_roundtrips_with_caret_position() {
        let text = "ab\ncd";
        let l = two_line_layout();
        // For each addressable byte, its caret-x must map back to that byte.
        for &(line_idx, byte) in &[(0usize, 0usize), (0, 1), (0, 2), (1, 3), (1, 4), (1, 5)] {
            let (_, x, _) = l.caret_position(byte);
            assert_eq!(
                l.byte_at_line_x(text, line_idx, x),
                byte,
                "x={x} on line {line_idx} should map back to byte {byte}"
            );
        }
    }

    #[test]
    fn byte_at_line_x_midpoint_rule() {
        let text = "ab\ncd";
        let l = two_line_layout();
        // line0 "ab": 'a' adv 5 (cluster 0), 'b' adv 6 (cluster 1), width 11.
        // Midpoint of 'a' is 2.5: x < 2.5 → byte0, x ≥ 2.5 → byte1.
        assert_eq!(l.byte_at_line_x(text, 0, 2.4), 0);
        assert_eq!(l.byte_at_line_x(text, 0, 2.6), 1);
    }
}
