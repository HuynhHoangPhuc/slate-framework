//! Multi-line paragraph layout with greedy word wrap.
//!
//! Provides `greedy_wrap` for breaking text into wrapped lines,
//! `compute_alignment_offset` for paint-time alignment, and
//! `truncate_with_ellipsis` for single-line truncation.

use crate::backend::{Font, TextBackend};
use crate::error::TextError;
use crate::types::{ShapedGlyph, ShapedLine, TextAlignment};

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

    // Shape a space to get space width (one call, reused every word boundary)
    let space_width = backend
        .shape_line(font, " ")
        .map(|s| s.width_lpx)
        .unwrap_or(0.0);

    let mut lines: Vec<ShapedLine> = Vec::new();
    // Accumulated glyphs for the current in-progress line
    let mut current_glyphs: Vec<ShapedGlyph> = Vec::new();
    let mut current_width = 0.0f32;
    // Metrics from the first word on the current line (used for ascent/descent)
    let mut current_ascent = metrics.ascent_lpx;
    let mut current_descent = metrics.descent_lpx;
    let mut y_offset = 0.0f32;

    for word in text.split_whitespace() {
        // Shape each word exactly once
        let shaped_word = backend.shape_line(font, word)?;
        let word_width = shaped_word.width_lpx;

        // Check if adding this word would exceed max_width
        let width_with_word = if current_glyphs.is_empty() {
            word_width
        } else {
            current_width + space_width + word_width
        };

        if width_with_word > max_width_lpx && !current_glyphs.is_empty() {
            // Wrap: finalize current line from accumulated glyphs
            let line = build_line_from_glyphs(
                current_glyphs,
                current_width,
                current_ascent,
                current_descent,
                y_offset,
            );
            lines.push(line);

            y_offset += line_height;
            current_glyphs = Vec::new();
            current_width = 0.0;
        }

        // Append word glyphs to current line, adjusting x offsets
        let pen_x = if current_glyphs.is_empty() {
            0.0
        } else {
            current_width + space_width
        };

        // First word on a fresh line: capture its metrics for ascent/descent
        if pen_x == 0.0 {
            current_ascent = shaped_word.ascent_lpx;
            current_descent = shaped_word.descent_lpx;
        }

        for g in &shaped_word.glyphs {
            let mut adjusted = *g;
            // Word was shape_line'd in isolation — positions start at 0.
            // Shift into the line by the current pen so they sit after the
            // preceding word + inter-word space.
            adjusted.position_lpx[0] += pen_x;
            current_glyphs.push(adjusted);
        }

        // Advance pen
        if pen_x == 0.0 {
            current_width = word_width;
        } else {
            current_width += space_width + word_width;
        }
    }

    // Finalize last line if not empty
    if !current_glyphs.is_empty() {
        let line = build_line_from_glyphs(
            current_glyphs,
            current_width,
            current_ascent,
            current_descent,
            y_offset,
        );
        lines.push(line);
    }

    Ok(lines)
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
