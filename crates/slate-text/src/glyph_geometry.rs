//! Byte ↔ pixel-x geometry helpers for caret math on shaped lines.
//!
//! Replaces the `.chars().count() * width` MVP that breaks on CJK ligatures,
//! Indic clusters, and emoji ZWJ sequences. Both helpers are byte-keyed end to
//! end and snap to grapheme boundaries so callers never have to.
//!
//! **Scope:** LTR text only. RTL/BiDi caret math is a later phase — these
//! helpers assume cluster values are non-decreasing in visual (= logical)
//! order, which only holds for LTR runs.

use crate::ShapedLine;
use unicode_segmentation::UnicodeSegmentation;

/// Pen-x position (line-relative, lpx) of the leading edge of the cluster
/// whose first byte is at `byte` in `text`.
///
/// `byte` is snapped to the nearest leading grapheme boundary (floor). Byte 0
/// maps to 0.0; `byte >= text.len()` maps to `line.width_lpx`.
pub fn pixel_x_at_byte(line: &ShapedLine, text: &str, byte: usize) -> f32 {
    if line.glyphs.is_empty() {
        return 0.0;
    }
    let snapped = snap_grapheme_floor(text, byte);
    if snapped == 0 {
        return 0.0;
    }
    if snapped >= text.len() {
        return line.width_lpx;
    }

    let mut pen = 0.0f32;
    for g in &line.glyphs {
        if (g.cluster as usize) >= snapped {
            return pen;
        }
        pen += g.x_advance_lpx;
    }
    line.width_lpx
}

/// Byte offset of the grapheme boundary nearest `x_lpx` (line-relative).
///
/// Mid-cluster x rounds to the leading/trailing edge by midpoint test:
/// `x_lpx < midpoint → leading boundary; else trailing boundary`.
/// `x_lpx <= 0` returns 0; `x_lpx >= line.width_lpx` returns `text.len()`.
pub fn byte_at_pixel_x(line: &ShapedLine, text: &str, x_lpx: f32) -> usize {
    if x_lpx <= 0.0 {
        return 0;
    }
    if x_lpx >= line.width_lpx {
        return text.len();
    }

    // Two sequential linear passes (O(N+G)), no nested glyph walk.
    //
    // Pass 1: build a byte-sorted table of (cluster_byte, pen_x) recording the
    // pen-x at the leading edge of each cluster. Clusters are non-decreasing in
    // LTR order, so an entry is pushed only when the cluster value increases;
    // pen_x is the running sum of advances of all earlier-cluster glyphs.
    let mut cluster_pen: Vec<(usize, f32)> = Vec::with_capacity(line.glyphs.len());
    let mut pen = 0.0f32;
    for g in &line.glyphs {
        let c = g.cluster as usize;
        match cluster_pen.last() {
            Some(&(last_c, _)) if last_c == c => {}
            _ => cluster_pen.push((c, pen)),
        }
        pen += g.x_advance_lpx;
    }

    // Pen-x at a grapheme boundary `byte`: the stored pen of the first cluster
    // entry whose cluster value is `>= byte` — i.e. the running advance over all
    // glyphs with `cluster < byte`, matching `pixel_x_at_byte`'s `cluster >=
    // snapped` early-return. If no cluster reaches `byte`, the boundary is past
    // every glyph → `line.width_lpx`. `cursor` advances monotonically across the
    // grapheme walk since both tables are byte-sorted → amortized O(1).
    let mut cursor = 0usize;
    let pen_at = |byte: usize, cursor: &mut usize| -> f32 {
        while *cursor < cluster_pen.len() && cluster_pen[*cursor].0 < byte {
            *cursor += 1;
        }
        if *cursor < cluster_pen.len() {
            cluster_pen[*cursor].1
        } else {
            line.width_lpx
        }
    };

    // Pass 2: merge-walk grapheme boundaries, applying the midpoint rule.
    let mut last_b = 0usize;
    let mut last_x = 0.0f32;
    for (b, _) in text.grapheme_indices(true) {
        if b == 0 {
            continue;
        }
        let x = pen_at(b, &mut cursor);
        if x_lpx < x {
            let mid = (last_x + x) * 0.5;
            return if x_lpx < mid { last_b } else { b };
        }
        last_b = b;
        last_x = x;
    }
    // Past the last interior boundary: split between last_b and text.len().
    let mid = (last_x + line.width_lpx) * 0.5;
    if x_lpx < mid { last_b } else { text.len() }
}

/// Largest grapheme-cluster boundary `<= byte` in `text`. Clamped to
/// `[0, text.len()]`.
fn snap_grapheme_floor(text: &str, byte: usize) -> usize {
    if byte >= text.len() {
        return text.len();
    }
    let mut last = 0usize;
    for (b, _) in text.grapheme_indices(true) {
        if b > byte {
            return last;
        }
        last = b;
    }
    last
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

    fn line(glyphs: Vec<ShapedGlyph>) -> ShapedLine {
        let width: f32 = glyphs.iter().map(|g| g.x_advance_lpx).sum();
        ShapedLine {
            glyphs,
            width_lpx: width,
            ascent_lpx: 10.0,
            descent_lpx: -2.0,
            y_offset_lpx: 0.0,
            base_direction: crate::types::Direction::Ltr,
            runs: Vec::new(),
        }
    }

    #[test]
    fn empty_line_returns_zero() {
        let l = line(vec![]);
        assert_eq!(pixel_x_at_byte(&l, "", 0), 0.0);
        assert_eq!(byte_at_pixel_x(&l, "", 0.0), 0);
    }

    #[test]
    fn ascii_boundaries_roundtrip() {
        // "abc" — 3 glyphs, clusters 0/1/2, advances 5/6/7.
        let l = line(vec![glyph(0, 5.0), glyph(1, 6.0), glyph(2, 7.0)]);
        assert_eq!(pixel_x_at_byte(&l, "abc", 0), 0.0);
        assert_eq!(pixel_x_at_byte(&l, "abc", 1), 5.0);
        assert_eq!(pixel_x_at_byte(&l, "abc", 2), 11.0);
        assert_eq!(pixel_x_at_byte(&l, "abc", 3), 18.0);
        assert_eq!(byte_at_pixel_x(&l, "abc", 0.0), 0);
        assert_eq!(byte_at_pixel_x(&l, "abc", 18.0), 3);
    }

    #[test]
    fn midpoint_rounding() {
        // Two glyphs, advance 10 each: 'a' (cluster 0), 'b' (cluster 1).
        let l = line(vec![glyph(0, 10.0), glyph(1, 10.0)]);
        // x < 5 → 0; x ≥ 5 → 1 (midpoint of first cluster).
        assert_eq!(byte_at_pixel_x(&l, "ab", 4.9), 0);
        assert_eq!(byte_at_pixel_x(&l, "ab", 5.0), 1);
        // x < 15 → 1; x ≥ 15 → 2 (midpoint of second cluster).
        assert_eq!(byte_at_pixel_x(&l, "ab", 14.9), 1);
        assert_eq!(byte_at_pixel_x(&l, "ab", 15.0), 2);
    }

    #[test]
    fn multi_glyph_cluster_decomposition() {
        // "é" encoded as e + combining acute (NFD): 3 bytes, 1 grapheme,
        // 2 glyphs both at cluster 0. Byte 3 (end) is the next boundary.
        let s = "e\u{0301}";
        let l = line(vec![glyph(0, 8.0), glyph(0, 0.0)]);
        assert_eq!(pixel_x_at_byte(&l, s, 0), 0.0);
        // Byte 1 snaps back to 0 (mid-grapheme): same pen-x as byte 0.
        assert_eq!(pixel_x_at_byte(&l, s, 1), 0.0);
        assert_eq!(pixel_x_at_byte(&l, s, 3), 8.0);
        // Round-trip on grapheme boundaries.
        assert_eq!(byte_at_pixel_x(&l, s, 0.0), 0);
        assert_eq!(byte_at_pixel_x(&l, s, 8.0), 3);
    }

    /// Reference impl of `byte_at_pixel_x` using the old per-boundary
    /// `pixel_x_at_byte` walk. Kept local to the test module so the O(N²)
    /// semantics stay verifiable after the production path was rewritten.
    fn byte_at_pixel_x_reference(line: &ShapedLine, text: &str, x_lpx: f32) -> usize {
        if x_lpx <= 0.0 {
            return 0;
        }
        if x_lpx >= line.width_lpx {
            return text.len();
        }
        let mut last_b = 0usize;
        let mut last_x = 0.0f32;
        for (b, _) in text.grapheme_indices(true) {
            if b == 0 {
                continue;
            }
            let x = pixel_x_at_byte(line, text, b);
            if x_lpx < x {
                let mid = (last_x + x) * 0.5;
                return if x_lpx < mid { last_b } else { b };
            }
            last_b = b;
            last_x = x;
        }
        let mid = (last_x + line.width_lpx) * 0.5;
        if x_lpx < mid { last_b } else { text.len() }
    }

    #[test]
    fn large_input_agreement() {
        // ~500 ASCII glyphs, ascending clusters, varied advances. Sweep x
        // across the whole line and assert the rewritten O(N+G) impl matches
        // the old per-boundary reference for every sample.
        let n = 500usize;
        let text: String = std::iter::repeat('a').take(n).collect();
        let glyphs: Vec<ShapedGlyph> = (0..n)
            .map(|i| glyph(i as u32, 3.0 + (i % 7) as f32))
            .collect();
        let l = line(glyphs);

        let mut x = -5.0f32;
        while x <= l.width_lpx + 5.0 {
            assert_eq!(
                byte_at_pixel_x(&l, &text, x),
                byte_at_pixel_x_reference(&l, &text, x),
                "mismatch at x={x}"
            );
            x += 0.37;
        }
    }

    #[test]
    fn snap_floor_basics() {
        assert_eq!(snap_grapheme_floor("abc", 0), 0);
        assert_eq!(snap_grapheme_floor("abc", 1), 1);
        assert_eq!(snap_grapheme_floor("abc", 3), 3);
        assert_eq!(snap_grapheme_floor("abc", 999), 3);
        // "é" NFD: graphemes start at 0 only; byte 1, 2 snap to 0.
        let s = "e\u{0301}";
        assert_eq!(snap_grapheme_floor(s, 1), 0);
        assert_eq!(snap_grapheme_floor(s, 2), 0);
        assert_eq!(snap_grapheme_floor(s, 3), 3);
    }
}
