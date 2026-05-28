//! Single-paragraph shape → wrap → paint pipeline.
//!
//! - `shape`: bidi/UAX-14 segmentation + per-word native shaping ([`ShapedWord`]).
//! - `wrap`: greedy first-fit and visual-line assembly into [`ShapedLine`].
//! - `paint`: paint-time post-processors (alignment offset, ellipsis truncation).
//!
//! The two-stage pipeline ([`shape_words`] + [`wrap_shaped_words`]) lets callers
//! re-fit on a width change without re-shaping. [`greedy_wrap`] chains both for
//! the one-shot path, and [`shape_line_bidi`] runs the single-line variant
//! through the same segment → reorder pipeline.

mod paint;
mod shape;
mod wrap;

use crate::backend::{Font, TextBackend};
use crate::error::TextError;
use crate::types::{Direction, ShapedLine};

pub use paint::{compute_alignment_offset, truncate_with_ellipsis};
pub use shape::{ShapedWord, shape_words};
pub use wrap::wrap_shaped_words;

pub(crate) use shape::shape_words_in;
pub(crate) use wrap::{assemble_visual_line, fit_advance};

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

/// Shape a single line through the bidi segment + reorder pipeline, producing a
/// run-bearing [`ShapedLine`] (the same path [`greedy_wrap`] / `shape_document`
/// use, minus the wrap fit).
///
/// Unlike the raw native `shape_line`, this resolves UAX #9 level-runs, shapes
/// each per its resolved direction, reorders into visual order, and populates
/// `runs` so the run-aware caret / hit-test math works on mixed / RTL text. On
/// pure-LTR / CJK input every item is level 0, so `assemble_visual_line` takes
/// its fast path: `runs` stays empty and glyph placement is byte-identical to
/// the pre-bidi single-line shaper (caret geometry then uses the original LTR
/// pen walk). The trade-off is that LTR text loses whole-line cross-word
/// kerning — consistent with the rest of the system, which shapes per segment.
///
/// Clusters are line-relative (the caller passes the line text as the whole
/// string); empty input yields a zero-glyph line carrying the font metrics.
pub fn shape_line_bidi<B: TextBackend>(
    backend: &B,
    font: &B::Font,
    text: &str,
) -> Result<ShapedLine, TextError> {
    let metrics = font.metrics();
    if text.is_empty() {
        return Ok(ShapedLine {
            glyphs: Vec::new(),
            width_lpx: 0.0,
            ascent_lpx: metrics.ascent_lpx,
            descent_lpx: metrics.descent_lpx,
            y_offset_lpx: 0.0,
            base_direction: Direction::Ltr,
            runs: Vec::new(),
        });
    }

    let space_width = backend
        .shape_line(font, " ")
        .map(|s| s.width_lpx)
        .unwrap_or(0.0);
    let tab_width = 4.0 * space_width;

    let items = shape_words_in(backend, font, text, 0)?;
    let item_refs: Vec<&ShapedWord> = items.iter().collect();
    Ok(assemble_visual_line(
        &item_refs,
        metrics.ascent_lpx,
        metrics.descent_lpx,
        0.0,
        true,
        tab_width,
    ))
}
