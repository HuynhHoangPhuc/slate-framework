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
use crate::types::{ShapedGlyph, ShapedLine, TextAlignment};

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
}

/// Shape every whitespace-delimited word in `text` exactly once.
///
/// Returns the per-word shaping results plus the shared inter-word space
/// advance (the space is shaped once). Pair with [`wrap_shaped_words`] to fit
/// the words to any width with zero further shaping calls — so re-wrap on a
/// resize is pure arithmetic.
///
/// Consecutive whitespace and newlines collapse (`split_whitespace`); empty or
/// whitespace-only input yields an empty word list.
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

/// Shape the whitespace-delimited words in `segment`, recording each word's
/// byte span as an absolute offset into the larger document (`segment_start` is
/// the byte offset of `segment` within that document; pass 0 when `segment` is
/// the whole text).
///
/// Shared by [`shape_words`] (single segment, offset 0) and the multi-line
/// paragraph shaper, which calls it once per `\n`-delimited paragraph so word
/// ranges stay absolute across the document.
pub(crate) fn shape_words_in<B: TextBackend>(
    backend: &B,
    font: &B::Font,
    segment: &str,
    segment_start: usize,
) -> Result<Vec<ShapedWord>, TextError> {
    let base = segment.as_ptr() as usize;
    let mut words = Vec::new();
    for word in segment.split_whitespace() {
        let shaped = backend.shape_line(font, word)?;
        // `word` is a subslice of `segment`; its pointer offset is the local
        // byte position, which we lift to an absolute document offset.
        let local = word.as_ptr() as usize - base;
        let start = segment_start + local;
        words.push(ShapedWord {
            glyphs: shaped.glyphs,
            advance_width_lpx: shaped.width_lpx,
            ascent_lpx: shaped.ascent_lpx,
            descent_lpx: shaped.descent_lpx,
            source_byte_range: start..start + word.len(),
        });
    }
    Ok(words)
}

/// Fit pre-shaped words into lines by greedy first-fit — pure width arithmetic,
/// no shaping calls.
///
/// Glyphs are concatenated with a per-word pen offset (the same join logic as
/// [`greedy_wrap`]); cross-word kerning across the space boundary is
/// intentionally dropped (see module docs). The first word on a line is always
/// accepted even if it overflows `max_width_lpx`. `line_height` is written into
/// each line's `y_offset_lpx` as the vertical advance between lines.
pub fn wrap_shaped_words(
    words: &[ShapedWord],
    space_width: f32,
    line_height: f32,
    max_width_lpx: f32,
) -> Vec<ShapedLine> {
    let mut lines: Vec<ShapedLine> = Vec::new();
    let mut current_glyphs: Vec<ShapedGlyph> = Vec::new();
    let mut current_width = 0.0f32;
    // Metrics from the first word on the current line (ascent/descent).
    let mut current_ascent = 0.0f32;
    let mut current_descent = 0.0f32;
    let mut y_offset = 0.0f32;

    for word in words {
        // Check if adding this word would exceed max_width.
        let width_with_word = if current_glyphs.is_empty() {
            word.advance_width_lpx
        } else {
            current_width + space_width + word.advance_width_lpx
        };

        if width_with_word > max_width_lpx && !current_glyphs.is_empty() {
            // Wrap: finalize current line from accumulated glyphs.
            lines.push(build_line_from_glyphs(
                std::mem::take(&mut current_glyphs),
                current_width,
                current_ascent,
                current_descent,
                y_offset,
            ));
            y_offset += line_height;
            current_width = 0.0;
        }

        let pen_x = if current_glyphs.is_empty() {
            0.0
        } else {
            current_width + space_width
        };

        // First word on a fresh line: capture its metrics for ascent/descent.
        if pen_x == 0.0 {
            current_ascent = word.ascent_lpx;
            current_descent = word.descent_lpx;
        }

        for g in &word.glyphs {
            let mut adjusted = *g;
            // Word glyphs start at pen 0; shift into the line by the running pen.
            adjusted.position_lpx[0] += pen_x;
            current_glyphs.push(adjusted);
        }

        if pen_x == 0.0 {
            current_width = word.advance_width_lpx;
        } else {
            current_width += space_width + word.advance_width_lpx;
        }
    }

    // Finalize last line if not empty.
    if !current_glyphs.is_empty() {
        lines.push(build_line_from_glyphs(
            current_glyphs,
            current_width,
            current_ascent,
            current_descent,
            y_offset,
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
/// - Consecutive spaces and newlines are collapsed (`split_whitespace`).
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
