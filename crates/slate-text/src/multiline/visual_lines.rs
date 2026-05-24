//! Caret byte ↔ visual-line resolution.

use super::MultilineLayout;

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
        self.caret_position_with_affinity(byte, crate::types::Affinity::Downstream)
    }

    /// Like [`Self::caret_position`] but resolves a direction-boundary byte to
    /// the visual edge that `affinity` selects (the trailing edge of the run
    /// ending at the byte, or the leading edge of the run starting there). The
    /// soft-wrap line-resolution rule in [`Self::line_for_byte`] is unaffected:
    /// affinity here picks only the within-line visual x on a mixed line.
    pub fn caret_position_with_affinity(
        &self,
        byte: usize,
        affinity: crate::types::Affinity,
    ) -> (usize, f32, f32) {
        if self.lines.is_empty() {
            return (0, 0.0, 0.0);
        }
        let idx = self.line_for_byte(byte);
        let vline = &self.lines[idx];
        // Run-bearing line (mixed / RTL): defer to the run-aware caret math so
        // the caret tracks the correct visual edge of a logical byte.
        if !vline.line.runs.is_empty() {
            let x = crate::glyph_geometry::run_caret_x_at_affinity(&vline.line, byte, affinity);
            return (idx, x, vline.line.y_offset_lpx);
        }
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

#[cfg(test)]
mod tests {
    use super::super::test_helpers::*;

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
}
