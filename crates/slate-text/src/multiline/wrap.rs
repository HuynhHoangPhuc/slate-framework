//! Document shaping + width-fitting (greedy first-fit + over-wide cluster break).

use std::ops::Range;

use super::{MultilineLayout, ShapedDocument, ShapedParagraph, VisualLine};
use crate::backend::{Font, TextBackend};
use crate::error::TextError;
use crate::paragraph::{ShapedWord, assemble_visual_line, fit_advance, shape_words_in};
use crate::types::{ShapedGlyph, ShapedLine};

/// Shape `text` into a [`ShapedDocument`]: split at UAX #14 mandatory breaks,
/// shape each paragraph's words once. Pair with [`wrap_document`] to fit to a
/// width.
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

    // Split at UAX #14 mandatory breaks (`\n`, `\r`, `\r\n`, VT, FF, NEL,
    // U+2028, U+2029), tracking each paragraph's absolute start. Coverage of a
    // paragraph runs to the *next* paragraph's start (folding in the trailing
    // terminator of any byte length), so ranges are gap-free; the final
    // paragraph ends at text.len().
    let spans = crate::linebreak::split_paragraphs(text);

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

    let total_len = doc.paragraphs.last().map(|p| p.byte_range.end).unwrap_or(0);

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
fn fit_paragraph<'a>(
    doc: &ShapedDocument,
    words: &'a [ShapedWord],
    max_width: f32,
    coverage: &Range<usize>,
    out: &mut Vec<(ShapedLine, usize)>,
) {
    let first_idx = out.len();
    let tab_width = 4.0 * doc.space_width;
    // Items committed to the current line in logical (source) order; glyphs are
    // placed in visual order — and word-local clusters rewritten to absolute —
    // only at line flush (`assemble_visual_line` with `rewrite_clusters_absolute`).
    let mut cur: Vec<&'a ShapedWord> = Vec::new();
    let mut cur_width = 0.0f32;
    let mut cur_start = 0usize;
    // A trailing space run held back: committed (visible) before a following
    // word that fits, or absorbed at a soft wrap / before an over-wide word.
    let mut pending: Option<&'a ShapedWord> = None;

    // Flush the current line's items into `out` (visual order, doc metrics,
    // absolute clusters). Caller guards against empty.
    let flush = |cur: &mut Vec<&'a ShapedWord>,
                 cur_start: usize,
                 out: &mut Vec<(ShapedLine, usize)>| {
        let line = assemble_visual_line(cur, doc.ascent_lpx, doc.descent_lpx, 0.0, true, tab_width);
        out.push((line, cur_start));
        cur.clear();
    };

    for word in words {
        if word.is_space_run {
            // Hold the run; the scanner never emits two runs back to back.
            pending = Some(word);
            continue;
        }

        // A word that cannot fit even on its own line is broken at grapheme
        // boundaries (tabs are pen-relative, never over-wide). The pending run
        // is a soft break → absorb it, flush the line, then emit the pieces.
        if !word.is_tab && word.advance_width_lpx > max_width {
            pending = None;
            if !cur.is_empty() {
                flush(&mut cur, cur_start, out);
                cur_width = 0.0;
            }
            for (glyphs, width, start) in break_word(word, max_width) {
                out.push((build_shaped_line(doc, glyphs, width), start));
            }
            continue;
        }

        let pending_w = pending.map(|s| s.advance_width_lpx).unwrap_or(0.0);
        let word_w = fit_advance(word, cur_width + pending_w, tab_width);
        let width_with = if cur.is_empty() {
            word_w
        } else {
            cur_width + pending_w + word_w
        };
        // Wrap only at an allowed UAX #14 opportunity (`break_before`); items
        // with no break before them ride out the overflow on the current line.
        if width_with > max_width && !cur.is_empty() && word.break_before {
            // Wrap before this word; absorb the pending run (no visible width
            // on either line) and start the next line with the word.
            flush(&mut cur, cur_start, out);
            cur_width = 0.0;
            pending = None;
            cur_start = word.source_byte_range.start;
            cur.push(word);
            cur_width += fit_advance(word, 0.0, tab_width);
        } else {
            if cur.is_empty() {
                // Line begins at the pending run's start (leading spaces stay
                // visible) or at the word if there is no pending run.
                cur_start = pending
                    .map(|s| s.source_byte_range.start)
                    .unwrap_or(word.source_byte_range.start);
            }
            if let Some(sp) = pending.take() {
                cur.push(sp);
                cur_width += sp.advance_width_lpx;
            }
            cur.push(word);
            cur_width += fit_advance(word, cur_width, tab_width);
        }
    }

    // Trailing/standalone spaces at the paragraph (hard line) end stay visible
    // and caret-addressable.
    if let Some(sp) = pending.take() {
        if cur.is_empty() {
            cur_start = sp.source_byte_range.start;
        }
        cur.push(sp);
    }
    if !cur.is_empty() {
        flush(&mut cur, cur_start, out);
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
            out.push(finish_sub(
                &mut sub,
                sub_origin_x,
                sub_width,
                word_start + sub_cluster as usize,
            ));
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
        out.push(finish_sub(
            &mut sub,
            sub_origin_x,
            sub_width,
            word_start + sub_cluster as usize,
        ));
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
