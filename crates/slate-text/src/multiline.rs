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

    for word in words {
        // A word that cannot fit even on its own line is broken at grapheme
        // boundaries. Flush the in-progress line first, then emit the pieces.
        if word.advance_width_lpx > max_width {
            if !cur.is_empty() {
                out.push((build_shaped_line(doc, std::mem::take(&mut cur), cur_width), cur_start));
                cur_width = 0.0;
            }
            for (glyphs, width, start) in break_word(word, max_width) {
                out.push((build_shaped_line(doc, glyphs, width), start));
            }
            continue;
        }

        let width_with = if cur.is_empty() {
            word.advance_width_lpx
        } else {
            cur_width + doc.space_width + word.advance_width_lpx
        };
        if width_with > max_width && !cur.is_empty() {
            out.push((build_shaped_line(doc, std::mem::take(&mut cur), cur_width), cur_start));
            cur_width = 0.0;
        }

        let pen_x = if cur.is_empty() {
            cur_start = word.source_byte_range.start;
            0.0
        } else {
            cur_width + doc.space_width
        };
        for g in &word.glyphs {
            let mut adjusted = *g;
            adjusted.position_lpx[0] += pen_x;
            // Rewrite the word-local cluster to a document-absolute byte offset
            // so caret/hit-test math over the line is byte-keyed end to end.
            adjusted.cluster = (word.source_byte_range.start + g.cluster as usize) as u32;
            cur.push(adjusted);
        }
        cur_width = if pen_x == 0.0 {
            word.advance_width_lpx
        } else {
            cur_width + doc.space_width + word.advance_width_lpx
        };
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
}
