//! Multi-line paragraph layout with greedy word wrap.
//!
//! Provides `greedy_wrap` for breaking text into wrapped lines,
//! `compute_alignment_offset` for paint-time alignment, and
//! `truncate_with_ellipsis` for single-line truncation.

use crate::backend::{Font, TextBackend};
use crate::error::TextError;
use crate::types::{ShapedLine, TextAlignment};

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

    // Shape a space to get space width
    let space_width = backend
        .shape_line(font, " ")
        .map(|s| s.width_lpx)
        .unwrap_or(0.0);

    let mut lines = Vec::new();
    let mut current_words: Vec<&str> = Vec::new();
    let mut current_width = 0.0f32;
    let mut y_offset = 0.0f32;

    for word in text.split_whitespace() {
        let shaped_word = backend.shape_line(font, word)?;
        let word_width = shaped_word.width_lpx;

        // Check if adding this word would exceed max_width
        let width_with_word = if current_words.is_empty() {
            word_width
        } else {
            current_width + space_width + word_width
        };

        if width_with_word > max_width_lpx && !current_words.is_empty() {
            // Wrap: finalize current line
            let line_text = current_words.join(" ");
            let mut line = backend.shape_line(font, &line_text)?;
            line.y_offset_lpx = y_offset;
            lines.push(line);

            y_offset += line_height;
            current_words.clear();
            current_width = 0.0;
        }

        // Add word to current line
        if current_words.is_empty() {
            current_width = word_width;
        } else {
            current_width += space_width + word_width;
        }
        current_words.push(word);
    }

    // Finalize last line if not empty
    if !current_words.is_empty() {
        let line_text = current_words.join(" ");
        let mut line = backend.shape_line(font, &line_text)?;
        line.y_offset_lpx = y_offset;
        lines.push(line);
    }

    Ok(lines)
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
/// Uses binary search to find the optimal cut point, then appends "..."
/// glyphs. Returns the original line unchanged if it fits.
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

    // Build truncated glyphs + ellipsis
    let mut truncated_glyphs = shaped.glyphs[..cut_idx].to_vec();
    let truncated_width = cumulative;

    // Adjust ellipsis glyph positions (they start at 0, shift by truncated width)
    for eg in &ellipsis.glyphs {
        let mut adjusted = *eg;
        adjusted.x_offset_lpx += truncated_width;
        truncated_glyphs.push(adjusted);
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
