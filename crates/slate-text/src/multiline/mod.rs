//! Byte-aware multi-line wrap for editable multi-line text (`TextArea`).
//!
//! `wrap_shaped_words` fits pre-shaped words to a width but discards which
//! source bytes landed on which visual line — the multi-line caret model needs
//! that mapping. This module adds it:
//!
//! - [`shape_document`] splits `text` at UAX #14 mandatory breaks (`\n`, `\r`,
//!   `\r\n`, VT, FF, NEL, U+2028, U+2029) into paragraphs, shapes each
//!   paragraph's words exactly once (reusing `shape_words_in`), and keeps the
//!   absolute byte coverage of each paragraph (the trailing terminator, of any
//!   byte length, is folded into the preceding paragraph so coverage is
//!   gap-free).
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
//!

use std::ops::Range;

use crate::paragraph::ShapedWord;
use crate::types::ShapedLine;

mod nav;
mod visual_lines;
mod wrap;

pub use wrap::{shape_document, wrap_document};

/// One paragraph's pre-shaped words plus its absolute byte coverage, delimited
/// by UAX #14 mandatory breaks. `byte_range` includes the paragraph's trailing
/// terminator of any byte length — `\n`, lone `\r`, `\r\n`, VT, FF, NEL, U+2028,
/// U+2029 (except the last paragraph, which ends at `text.len()`), so
/// concatenated paragraph ranges tile `0..text.len()` with no gaps.
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

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;
    use crate::types::{FontId, ShapedGlyph};

    pub fn glyph(cluster: u32, adv: f32) -> ShapedGlyph {
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

    pub fn vline(
        glyphs: Vec<ShapedGlyph>,
        byte_start: usize,
        byte_end: usize,
        y: f32,
    ) -> VisualLine {
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
    pub fn two_line_layout() -> MultilineLayout {
        MultilineLayout {
            lines: vec![
                vline(vec![glyph(0, 5.0), glyph(1, 6.0)], 0, 3, 0.0),
                vline(vec![glyph(3, 7.0), glyph(4, 8.0)], 3, 5, 12.0),
            ],
            total_height_lpx: 24.0,
            line_height_lpx: 12.0,
        }
    }
}
