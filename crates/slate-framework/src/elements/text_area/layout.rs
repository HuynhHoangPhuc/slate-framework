//! TextArea layout construction + pointer/selection geometry.
//!
//! [`build_layout`] shapes the document once and fits it to a fixed content
//! width, then floors the element height to `min_lines` rows so an empty or
//! short field still reserves space. The expensive shaping happens here;
//! `paint` reuses the returned [`MultilineLayout`] for glyph placement and
//! caret math — no re-shaping per frame.
//!
//! [`byte_at_point`] and [`selection_rects`] are the paint/handler-side
//! geometry that turns a window-relative pointer position into a caret byte and
//! a selection range into per-visual-line highlight rectangles. Both read the
//! cached [`MultilineLayout`] and the painted origin; neither re-shapes.

use std::rc::Rc;

use slate_text::{MultilineLayout, ShapedLine, TextError};

use crate::text_system::{PlatformFont, TextSystem};

/// Build the wrapped multi-line layout for `text` at `width_lpx`, returning the
/// shared layout plus the element height floored to `min_lines` rows.
///
/// `min_lines` of 0 is treated as 1 so the floor never collapses the field to
/// zero height (a layout always has ≥ 1 visual line).
pub(crate) fn build_layout(
    text_system: &TextSystem,
    font: &PlatformFont,
    text: &str,
    width_lpx: f32,
    min_lines: usize,
) -> Result<(Rc<MultilineLayout>, f32), TextError> {
    let doc = text_system.shape_document(font, text)?;
    let layout = slate_text::wrap_document(&doc, width_lpx);

    let floor_rows = min_lines.max(1) as f32;
    let height = layout.total_height_lpx.max(floor_rows * layout.line_height_lpx);

    Ok((Rc::new(layout), height))
}

/// Map a window-relative pointer `(x, y)` to an absolute caret byte offset.
///
/// `paint_origin_{x,y}` are the element's painted top-left (cached on
/// `ImeState` during paint). The y coordinate selects a visual line by the
/// uniform line height; the x coordinate is resolved within that line by
/// [`MultilineLayout::byte_at_line_x`] (clamped to the line's addressable
/// range). Clicks above the first line clamp to byte 0; clicks below the last
/// line clamp to the document end. Returns 0 for an empty layout.
pub(crate) fn byte_at_point(
    layout: &MultilineLayout,
    text: &str,
    paint_origin_x: f32,
    paint_origin_y: f32,
    x: f32,
    y: f32,
) -> usize {
    if layout.lines.is_empty() {
        return 0;
    }
    let local_y = y - paint_origin_y;
    if local_y < 0.0 {
        return 0; // above the first line → document start
    }
    if local_y >= layout.total_height_lpx {
        // below the last line → document end (last line's caret-addressable end)
        let last = layout.lines.len() - 1;
        return layout.line_caret_end(text, last);
    }

    let lh = layout.line_height_lpx;
    let line_idx = if lh > 0.0 {
        ((local_y / lh) as usize).min(layout.lines.len() - 1)
    } else {
        0
    };
    let local_x = x - paint_origin_x;
    layout.byte_at_line_x(text, line_idx, local_x)
}

/// One selection highlight rectangle, element-relative (add the paint origin at
/// draw time). `width_lpx` is already clamped to ≥ 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SelectionRect {
    /// Left edge, relative to the element's painted x-origin.
    pub x_lpx: f32,
    /// Top edge, relative to the element's painted y-origin (the line's
    /// `y_offset_lpx`).
    pub y_lpx: f32,
    /// Rectangle width in logical pixels.
    pub width_lpx: f32,
}

/// Build one highlight rect per visual line intersecting the byte range
/// `[lo, hi)`.
///
/// - The line where the selection starts mid-way runs from the start byte's x
///   to the line's text end (`width_lpx`).
/// - A line fully covered (selection spans from at-or-before its `byte_start`
///   to at-or-after its `byte_end`) extends to `content_width` so the wrapped
///   break / newline reads as selected.
/// - The line where the selection ends mid-way runs from the line start to the
///   end byte's x.
///
/// Empty selections (`lo >= hi`) and zero-width rects produce nothing.
pub(crate) fn selection_rects(
    layout: &MultilineLayout,
    lo: usize,
    hi: usize,
    content_width: f32,
) -> Vec<SelectionRect> {
    let mut rects = Vec::new();
    if lo >= hi {
        return rects;
    }

    for vline in &layout.lines {
        // Skip lines entirely outside the selection. A line whose byte_start
        // equals hi (the start of the line after the selection's end) is
        // excluded — nothing on it is selected.
        if vline.byte_end <= lo || vline.byte_start >= hi {
            continue;
        }

        let line_lo = lo.max(vline.byte_start);
        let x_start = line_x_at_byte(&vline.line, line_lo);

        let starts_at_head = lo <= vline.byte_start;
        let covers_to_end = hi >= vline.byte_end;
        let x_end = if starts_at_head && covers_to_end {
            // Fully-covered interior line → full content width signals the wrap.
            content_width
        } else {
            // Partial line: clamp the end byte to this line's coverage.
            line_x_at_byte(&vline.line, hi.min(vline.byte_end))
        };

        let width = (x_end - x_start).max(0.0);
        if width > 0.0 {
            rects.push(SelectionRect {
                x_lpx: x_start,
                y_lpx: vline.line.y_offset_lpx,
                width_lpx: width,
            });
        }
    }
    rects
}

/// Build the IME display string by splicing the active composition into the
/// committed text at the caret: `committed[..caret] + preedit + committed[caret..]`.
///
/// `caret` is clamped to `committed.len()` and floored to the nearest char
/// boundary at or below it, so a caret that momentarily indexes mid-codepoint
/// (or past the end) never panics the splice. An empty `preedit` short-circuits
/// to a clone of `committed` — paint uses the pre-shaped layout in that case.
pub(crate) fn compose_display(committed: &str, caret: usize, preedit: &str) -> String {
    if preedit.is_empty() {
        return committed.to_string();
    }
    let mut at = caret.min(committed.len());
    while at > 0 && !committed.is_char_boundary(at) {
        at -= 1;
    }
    let mut out = String::with_capacity(committed.len() + preedit.len());
    out.push_str(&committed[..at]);
    out.push_str(preedit);
    out.push_str(&committed[at..]);
    out
}

/// One preedit run on a single visual line: the line it sits on plus the
/// element-relative x and width of the composed bytes covered on that line.
/// Paint resolves the y (underline below baseline, highlight at line top) from
/// the line's own metrics, so this carries only the horizontal geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PreeditRun {
    /// Index into `layout.lines`.
    pub line_idx: usize,
    /// Left edge of the run, relative to the element's painted x-origin.
    pub x_lpx: f32,
    /// Run width in logical pixels (hugs the composed glyphs, never extends to
    /// the content edge the way a wrapped *selection* rect does).
    pub width_lpx: f32,
}

/// Split the composed byte range `[start, hi)` of a preedit run into one
/// per-visual-line run, hugging the glyphs actually on each line.
///
/// Unlike [`selection_rects`], a fully-covered interior line is **not** widened
/// to `content_width`: an IME underline / target highlight tracks the composed
/// text, not the wrap fill. Empty ranges and zero-width runs produce nothing.
pub(crate) fn preedit_runs(layout: &MultilineLayout, start: usize, end: usize) -> Vec<PreeditRun> {
    let mut runs = Vec::new();
    if start >= end {
        return runs;
    }
    for (idx, vline) in layout.lines.iter().enumerate() {
        if vline.byte_end <= start || vline.byte_start >= end {
            continue;
        }
        let lo = start.max(vline.byte_start);
        let hi = end.min(vline.byte_end);
        let x_start = line_x_at_byte(&vline.line, lo);
        let x_end = line_x_at_byte(&vline.line, hi);
        let width = (x_end - x_start).max(0.0);
        if width > 0.0 {
            runs.push(PreeditRun {
                line_idx: idx,
                x_lpx: x_start,
                width_lpx: width,
            });
        }
    }
    runs
}

/// Pen-x of the leading edge of the cluster at absolute `byte` on `line`:
/// the summed advance of every glyph whose (document-absolute) cluster is
/// strictly before `byte`. Returns the line width for a byte past the last
/// glyph. Mirrors `MultilineLayout::caret_position`'s pen walk.
fn line_x_at_byte(line: &ShapedLine, byte: usize) -> f32 {
    let mut pen = 0.0f32;
    for g in &line.glyphs {
        if g.cluster as usize >= byte {
            return pen;
        }
        pen += g.x_advance_lpx;
    }
    line.width_lpx
}

#[cfg(test)]
mod tests {
    use super::*;
    use slate_text::{FontHandle, FontId, ShapedGlyph, VisualLine};

    fn glyph(cluster: u32, adv: f32) -> ShapedGlyph {
        ShapedGlyph {
            glyph_id: 1,
            font_id: FontId::PRIMARY,
            font_handle: FontHandle::default(),
            x_advance_lpx: adv,
            position_lpx: [0.0, 0.0],
            cluster,
        }
    }

    fn vline(glyphs: Vec<ShapedGlyph>, byte_start: usize, byte_end: usize, y: f32) -> VisualLine {
        let width: f32 = glyphs.iter().map(|g| g.x_advance_lpx).sum();
        VisualLine {
            line: ShapedLine {
                glyphs,
                width_lpx: width,
                ascent_lpx: 10.0,
                descent_lpx: -2.0,
                y_offset_lpx: y,
            },
            byte_start,
            byte_end,
        }
    }

    /// "ab\ncd": line0 "ab" (clusters 0/1, adv 5/6, width 11, covers the '\n' so
    /// byte_end=3), line1 "cd" (clusters 3/4, adv 7/8, width 15). line_height 12.
    fn layout_abcd() -> MultilineLayout {
        MultilineLayout {
            lines: vec![
                vline(vec![glyph(0, 5.0), glyph(1, 6.0)], 0, 3, 0.0),
                vline(vec![glyph(3, 7.0), glyph(4, 8.0)], 3, 5, 12.0),
            ],
            total_height_lpx: 24.0,
            line_height_lpx: 12.0,
        }
    }

    /// Three lines of three/two/three clusters at advance 10, content gaps
    /// folded into each line's byte_end (so ranges tile 0..10):
    /// line0 "abc" 0..4 (covers '\n'), line1 "de" 4..7, line2 "fgh" 7..10.
    fn layout_3lines() -> MultilineLayout {
        MultilineLayout {
            lines: vec![
                vline(vec![glyph(0, 10.0), glyph(1, 10.0), glyph(2, 10.0)], 0, 4, 0.0),
                vline(vec![glyph(4, 10.0), glyph(5, 10.0)], 4, 7, 12.0),
                vline(vec![glyph(7, 10.0), glyph(8, 10.0), glyph(9, 10.0)], 7, 10, 24.0),
            ],
            total_height_lpx: 36.0,
            line_height_lpx: 12.0,
        }
    }

    #[test]
    fn byte_at_point_resolves_line_and_column() {
        let text = "ab\ncd";
        let l = layout_abcd();
        // Paint origin at (100, 50). A point in line0's y band, x near 'b' edge.
        // line0 'a' adv5 'b' adv6 → caret-x for byte 2 is 11; clicking past that
        // (local_x ≈ 11) snaps to line0 caret-end (byte 2).
        assert_eq!(byte_at_point(&l, text, 100.0, 50.0, 100.0 + 20.0, 50.0 + 3.0), 2);
        // y in line0 band, far-left x → line0 start (byte 0).
        assert_eq!(byte_at_point(&l, text, 100.0, 50.0, 100.0 - 5.0, 50.0 + 3.0), 0);
        // y in line1 band → a byte on line1 (≥ 3).
        let b = byte_at_point(&l, text, 100.0, 50.0, 100.0 + 1.0, 50.0 + 15.0);
        assert_eq!(b, 3, "left edge of line1 → its byte_start");
    }

    #[test]
    fn byte_at_point_clamps_y_out_of_band() {
        let text = "ab\ncd";
        let l = layout_abcd();
        // y past total height → document end (byte 5).
        assert_eq!(byte_at_point(&l, text, 100.0, 50.0, 100.0 + 100.0, 50.0 + 999.0), 5);
        // Negative y (above first line) → 0.
        assert_eq!(byte_at_point(&l, text, 100.0, 50.0, 100.0 + 100.0, 50.0 - 999.0), 0);
    }

    #[test]
    fn byte_at_point_line_boundary_y_picks_lower_line() {
        let text = "ab\ncd";
        let l = layout_abcd();
        // local_y exactly == line_height (12) is the top of line1 → resolve to
        // line1 (consistent with the single no-affinity rule). x far-left → 3.
        assert_eq!(byte_at_point(&l, text, 0.0, 0.0, -5.0, 12.0), 3);
    }

    #[test]
    fn selection_rects_three_line_span() {
        // Select bytes 1..9 across all three lines.
        let l = layout_3lines();
        let rects = selection_rects(&l, 1, 9, 100.0);
        assert_eq!(rects.len(), 3, "one rect per spanned line");
        // line0: starts mid-line at byte1 (x=10) → to line text end (width 30).
        assert_eq!(rects[0], SelectionRect { x_lpx: 10.0, y_lpx: 0.0, width_lpx: 20.0 });
        // line1: fully covered interior → full content width (100) from x=0.
        assert_eq!(rects[1], SelectionRect { x_lpx: 0.0, y_lpx: 12.0, width_lpx: 100.0 });
        // line2: from line start (x=0) to byte9 (x=20).
        assert_eq!(rects[2], SelectionRect { x_lpx: 0.0, y_lpx: 24.0, width_lpx: 20.0 });
    }

    #[test]
    fn selection_rects_single_line() {
        // "abc" single-line parity with the TextField path: one rect lo_x..hi_x.
        let l = MultilineLayout {
            lines: vec![vline(
                vec![glyph(0, 10.0), glyph(1, 10.0), glyph(2, 10.0), glyph(3, 10.0)],
                0,
                4,
                0.0,
            )],
            total_height_lpx: 12.0,
            line_height_lpx: 12.0,
        };
        let rects = selection_rects(&l, 1, 3, 100.0);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0], SelectionRect { x_lpx: 10.0, y_lpx: 0.0, width_lpx: 20.0 });
    }

    #[test]
    fn selection_rects_normalized_range_is_direction_agnostic() {
        // The caller normalizes to (lo, hi); a right-to-left drag yields the same
        // (lo, hi) and therefore identical rects.
        let l = layout_3lines();
        let ltr = selection_rects(&l, 1, 9, 100.0);
        let rtl = selection_rects(&l, 1, 9, 100.0);
        assert_eq!(ltr, rtl);
    }

    #[test]
    fn selection_rects_empty_selection_is_zero_rects() {
        let l = layout_3lines();
        assert!(selection_rects(&l, 4, 4, 100.0).is_empty());
        assert!(selection_rects(&l, 5, 2, 100.0).is_empty());
    }

    #[test]
    fn compose_display_inserts_at_caret() {
        assert_eq!(compose_display("ab", 0, "XY"), "XYab");
        assert_eq!(compose_display("ab", 1, "XY"), "aXYb");
        assert_eq!(compose_display("ab", 2, "XY"), "abXY");
    }

    #[test]
    fn compose_display_empty_preedit_is_clone() {
        assert_eq!(compose_display("hello", 3, ""), "hello");
        assert_eq!(compose_display("", 0, ""), "");
    }

    #[test]
    fn compose_display_clamps_and_floors_caret() {
        // Caret past end clamps to len.
        assert_eq!(compose_display("ab", 99, "X"), "abX");
        // Caret mid-codepoint floors to the char boundary below it ("é" = 2 bytes).
        assert_eq!(compose_display("é", 1, "X"), "Xé");
    }

    #[test]
    fn compose_display_multibyte_preedit_and_text() {
        // Caret 2 == after "ab"; insert CJK composition.
        assert_eq!(compose_display("ab", 2, "你好"), "ab你好");
        assert_eq!(compose_display("ab", 0, "你好"), "你好ab");
    }

    #[test]
    fn preedit_runs_hug_glyphs_not_content_width() {
        // Select bytes 1..9 across all three lines (same layout selection uses).
        let l = layout_3lines();
        let runs = preedit_runs(&l, 1, 9);
        assert_eq!(runs.len(), 3, "one run per spanned line");
        // line0: byte1 (x=10) .. min(9, byte_end=4) → byte4 maps to width 30.
        assert_eq!(runs[0], PreeditRun { line_idx: 0, x_lpx: 10.0, width_lpx: 20.0 });
        // line1 fully covered: hugs glyphs (x=0 .. width 20), NOT content_width(100).
        assert_eq!(runs[1], PreeditRun { line_idx: 1, x_lpx: 0.0, width_lpx: 20.0 });
        // line2: x=0 .. byte9 (x=20).
        assert_eq!(runs[2], PreeditRun { line_idx: 2, x_lpx: 0.0, width_lpx: 20.0 });
    }

    #[test]
    fn preedit_runs_empty_range_is_none() {
        let l = layout_3lines();
        assert!(preedit_runs(&l, 4, 4).is_empty());
        assert!(preedit_runs(&l, 9, 2).is_empty());
    }

    #[test]
    fn selection_rects_to_line_break_is_full_width() {
        // Selecting "ab\n" (lo=0, hi=3) covers line0 head-to-end → full width.
        let l = layout_abcd();
        let rects = selection_rects(&l, 0, 3, 200.0);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0], SelectionRect { x_lpx: 0.0, y_lpx: 0.0, width_lpx: 200.0 });
    }
}
