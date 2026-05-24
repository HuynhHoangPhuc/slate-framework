//! Caret end-of-line + line-relative x → byte hit-test (vertical nav, Home/End).

use unicode_segmentation::UnicodeSegmentation;

use super::MultilineLayout;

impl MultilineLayout {
    /// Absolute byte one past the last *visible* glyph cluster on line `idx` —
    /// the caret-addressable end of that visual line. Excludes a trailing hard
    /// `\n` and the whitespace absorbed at a soft-wrap boundary, so End and
    /// vertical motion keep the caret on the line rather than slipping to the
    /// next line's head (which the no-affinity rule would otherwise do). An
    /// empty line resolves to its own `byte_start`.
    pub fn line_caret_end(&self, text: &str, idx: usize) -> usize {
        match self.lines.get(idx) {
            Some(vline) => {
                // Run-bearing line: the last *logical* visible byte is the max
                // run end (visual order is reordered, so the visually-last glyph
                // is not the logical end). Runs already exclude a trailing hard
                // `\n` (it is not shaped), so this is the caret-addressable end.
                if !vline.line.runs.is_empty() {
                    return vline
                        .line
                        .runs
                        .iter()
                        .map(|r| r.byte_range.end)
                        .max()
                        .unwrap_or(vline.byte_start);
                }
                match vline.line.glyphs.last() {
                    Some(g) => next_grapheme(text, g.cluster as usize),
                    None => vline.byte_start,
                }
            }
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
        // Run-bearing line (mixed / RTL): the run-aware hit-test resolves the
        // logical byte directly. Clamp into the line's addressable range so
        // vertical motion never escapes to a neighbouring line.
        if !vline.line.runs.is_empty() {
            return crate::glyph_geometry::run_byte_at_x(&vline.line, x_lpx).clamp(start, end);
        }
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
    use super::super::MultilineLayout;
    use super::super::test_helpers::*;

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
