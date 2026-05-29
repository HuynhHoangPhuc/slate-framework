//! Multi-line caret motions: vertical nav, visual-line Home/End, newline.
//!
//! These are the only motions that need the wrapped visual-line model — every
//! other editing op (←/→/Backspace/typing/clipboard/undo) is byte-offset based
//! and reuses `text_edit` unchanged. Each function takes the `MultilineLayout`
//! the previous paint cached on `ImeState` (passed in by the handler as a
//! separate `Rc` clone so it does not alias the `&mut ImeState` borrow).
//!
//! Sticky column (`desired_x`): ↑/↓ seed it once from the caret's pixel-x and
//! reuse it on every subsequent vertical press, so the caret keeps its column
//! across shorter lines. Any other caret motion clears it.

use slate_text::MultilineLayout;

use crate::elements::text_edit::grapheme::insert_text_at;
use crate::elements::text_edit::ops::{
    apply_motion_to, apply_visual_edge, delete_selection, record_edit, reset_blink,
};
use crate::elements::text_edit::undo::EditOp;
use crate::ime::ImeState;

/// Re-arm blink and start a fresh undo group after a caret-only motion (so the
/// next edit coalesces into a new undo entry, mirroring TextField motions).
fn finish_motion(state: &mut ImeState) {
    reset_blink(state);
    state.undo.mark_motion();
}

/// Move the caret to the visual line above/below, preserving its column via the
/// sticky `desired_x`. Clamps at the document edges: ↑ on the first line jumps
/// to that line's start, ↓ on the last line jumps to its caret-end. `shift`
/// extends the selection (anchor at the pre-move caret).
pub(crate) fn move_vertical(
    state: &mut ImeState,
    layout: &MultilineLayout,
    down: bool,
    shift: bool,
) {
    if layout.lines.is_empty() {
        return;
    }
    // Seed the sticky column from the caret's *visual* x (affinity-aware so a
    // boundary caret seeds the column on the side it currently renders).
    let (line_idx, x, _) = layout.caret_position_with_affinity(state.caret, state.caret_affinity);
    let sticky = state.desired_x.unwrap_or(x);
    state.desired_x = Some(sticky);
    // Vertical motion lands on a fresh line; the boundary affinity does not
    // carry across lines, so reset to the default.
    state.caret_affinity = slate_text::Affinity::Downstream;

    let last = layout.lines.len() - 1;
    let target_line = if down {
        if line_idx >= last {
            let end = layout.line_caret_end(&state.text, last);
            apply_motion_to(state, shift, end);
            finish_motion(state);
            return;
        }
        line_idx + 1
    } else {
        if line_idx == 0 {
            let start = layout.lines[0].byte_start;
            apply_motion_to(state, shift, start);
            finish_motion(state);
            return;
        }
        line_idx - 1
    };

    let target_byte = layout.byte_at_line_x(&state.text, target_line, sticky);
    apply_motion_to(state, shift, target_byte);
    finish_motion(state);
}

/// Move the caret to the start (`to_end = false`) or caret-addressable end
/// (`to_end = true`) of its current visual line — NOT the document edges.
/// `shift` extends the selection. Clears the sticky column.
pub(crate) fn move_line_edge(
    state: &mut ImeState,
    layout: &MultilineLayout,
    to_end: bool,
    shift: bool,
) {
    if layout.lines.is_empty() {
        return;
    }
    let line_idx = layout.line_for_byte(state.caret);
    // Run-bearing line: Home/End go to the visual (screen) line edges, which on
    // a mixed/RTL line differ from the logical start/end byte.
    let handled = match layout.lines.get(line_idx) {
        Some(vline) if !vline.line.runs.is_empty() => {
            apply_visual_edge(state, &vline.line, to_end, shift)
        }
        _ => false,
    };
    if !handled {
        let target = if to_end {
            layout.line_caret_end(&state.text, line_idx)
        } else {
            layout.lines[line_idx].byte_start
        };
        apply_motion_to(state, shift, target);
        state.caret_affinity = slate_text::Affinity::Downstream;
    }
    state.desired_x = None;
    finish_motion(state);
}

/// Cross from a run-bearing line's *visual* edge to the adjacent line when a
/// visual ←/→ step had nowhere left to go on the current line. Logical grapheme
/// motion can't be used here: on a mixed line the logical neighbour of the
/// visual-edge byte sits back *inside* the line, so a plain fallback would step
/// the caret the wrong way instead of advancing to the next line.
///
/// Entry edge mirrors the press direction: moving right enters the next line at
/// its visual-left edge; moving left enters the previous line at its
/// visual-right edge. Clamps (no move) at the document edges. `shift` extends
/// the selection from the anchor `apply_visual_motion` already set.
pub(crate) fn visual_cross_line(
    state: &mut ImeState,
    layout: &MultilineLayout,
    move_right: bool,
    shift: bool,
) {
    let idx = layout.line_for_byte(state.caret);
    let target_idx = if move_right {
        if idx + 1 >= layout.lines.len() {
            return; // last line: clamp at the current visual edge.
        }
        idx + 1
    } else {
        if idx == 0 {
            return; // first line: clamp.
        }
        idx - 1
    };
    let Some(vline) = layout.lines.get(target_idx) else {
        return;
    };
    // Right → enter the next line from its left (Home edge); left → enter the
    // previous line from its right (End edge).
    let enter_to_end = !move_right;
    let (target, affinity) = if !vline.line.runs.is_empty() {
        slate_text::visual_line_edge(&vline.line, enter_to_end)
            .unwrap_or((vline.byte_start, slate_text::Affinity::Downstream))
    } else if enter_to_end {
        (
            layout.line_caret_end(&state.text, target_idx),
            slate_text::Affinity::Downstream,
        )
    } else {
        (vline.byte_start, slate_text::Affinity::Downstream)
    };
    apply_motion_to(state, shift, target);
    state.caret_affinity = affinity;
}

/// Insert a `\n` at the caret, replacing any selection. Records a discrete undo
/// step (a newline is its own undoable edit), clears the sticky column, and
/// returns the new buffer for the caller to push to the bound signal.
pub(crate) fn insert_newline(state: &mut ImeState) -> String {
    delete_selection(state);
    let caret = state.caret;
    state.caret = insert_text_at(&mut state.text, caret, "\n");
    record_edit(state, EditOp::Discrete);
    reset_blink(state);
    state.desired_x = None;
    state.caret_affinity = slate_text::Affinity::Downstream;
    state.text.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use slate_text::{Affinity, Direction, FontId, RunSpan, ShapedGlyph, ShapedLine};
    use slate_text::{MultilineLayout, VisualLine};

    fn glyph(cluster: u32, adv: f32) -> ShapedGlyph {
        dglyph(cluster, adv, Direction::Ltr)
    }

    fn dglyph(cluster: u32, adv: f32, dir: Direction) -> ShapedGlyph {
        ShapedGlyph {
            glyph_id: 1,
            font_id: FontId::PRIMARY,
            font_handle: slate_text::FontHandle::default(),
            x_advance_lpx: adv,
            position_lpx: [0.0, 0.0],
            cluster,
            direction: dir,
        }
    }

    fn run(range: std::ops::Range<usize>, dir: Direction) -> RunSpan {
        RunSpan {
            level: if dir == Direction::Rtl { 1 } else { 0 },
            byte_range: range,
            direction: dir,
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
                base_direction: Direction::Ltr,
                runs: Vec::new(),
            },
            byte_start,
            byte_end,
        }
    }

    /// "ab\ncd": line0 "ab" (clusters 0/1, adv 5/6), line1 "cd" (clusters 3/4,
    /// adv 7/8).
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

    fn state_with(text: &str, caret: usize) -> ImeState {
        ImeState {
            text: text.to_string(),
            caret,
            ..Default::default()
        }
    }

    /// "abאב\ncd": line0 mixed ("ab" LTR 0..2, "אב" RTL 2..6; visual stops L→R:
    /// 0,1,6,4,2), line1 "cd" LTR (bytes 7..9). Line0 spans 0..7 (incl. '\n').
    fn layout_mixed_then_ltr() -> MultilineLayout {
        let mixed = VisualLine {
            line: ShapedLine {
                glyphs: vec![
                    dglyph(0, 5.0, Direction::Ltr),
                    dglyph(1, 6.0, Direction::Ltr),
                    dglyph(4, 7.0, Direction::Rtl),
                    dglyph(2, 8.0, Direction::Rtl),
                ],
                width_lpx: 26.0,
                ascent_lpx: 10.0,
                descent_lpx: -2.0,
                y_offset_lpx: 0.0,
                base_direction: Direction::Rtl,
                runs: vec![run(0..2, Direction::Ltr), run(2..6, Direction::Rtl)],
            },
            byte_start: 0,
            byte_end: 7,
        };
        MultilineLayout {
            lines: vec![mixed, vline(vec![glyph(7, 5.0), glyph(8, 6.0)], 7, 9, 12.0)],
            total_height_lpx: 24.0,
            line_height_lpx: 12.0,
        }
    }

    #[test]
    fn vertical_down_preserves_column_and_seeds_desired_x() {
        let layout = layout_abcd();
        // Caret after 'b' on line0 (byte 2, x = 11). ↓ should land near x=11 on
        // line1 → byte 5 ("cd" end), and seed desired_x once.
        let mut s = state_with("ab\ncd", 2);
        move_vertical(&mut s, &layout, true, false);
        assert_eq!(s.desired_x, Some(11.0));
        assert_eq!(s.caret, 5);
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn vertical_sticky_survives_second_move_and_resets_after_horizontal() {
        let layout = layout_abcd();
        // Start mid-line0 (byte 1, x=5). ↓ to line1, then back ↑ should restore
        // the original column (byte 1) because desired_x is sticky.
        let mut s = state_with("ab\ncd", 1);
        move_vertical(&mut s, &layout, true, false);
        let after_down = s.caret;
        assert_eq!(s.desired_x, Some(5.0));
        move_vertical(&mut s, &layout, false, false);
        assert_eq!(s.caret, 1, "↑ restores the seeded column");
        assert_ne!(after_down, 1);
        // A line-edge move clears the sticky column.
        move_line_edge(&mut s, &layout, false, false);
        assert_eq!(s.desired_x, None);
    }

    #[test]
    fn vertical_clamps_at_edges() {
        let layout = layout_abcd();
        // ↑ on the first line → line start (byte 0), no panic.
        let mut s = state_with("ab\ncd", 1);
        move_vertical(&mut s, &layout, false, false);
        assert_eq!(s.caret, 0);
        // ↓ on the last line → line caret-end (byte 5).
        let mut s = state_with("ab\ncd", 4);
        move_vertical(&mut s, &layout, true, false);
        assert_eq!(s.caret, 5);
    }

    #[test]
    fn shift_vertical_extends_selection() {
        let layout = layout_abcd();
        let mut s = state_with("ab\ncd", 2);
        move_vertical(&mut s, &layout, true, true);
        assert_eq!(s.selection_anchor, Some(2), "anchor at pre-move caret");
        assert_eq!(s.caret, 5);
    }

    #[test]
    fn line_edge_is_visual_line_relative() {
        let layout = layout_abcd();
        // Caret on line1: Home → line1.byte_start (3), End → 5 (not doc 0 / len).
        let mut s = state_with("ab\ncd", 4);
        move_line_edge(&mut s, &layout, false, false);
        assert_eq!(s.caret, 3);
        move_line_edge(&mut s, &layout, true, false);
        assert_eq!(s.caret, 5);
        // Caret on line0: End → 2 (before the '\n'), not document end.
        let mut s = state_with("ab\ncd", 0);
        move_line_edge(&mut s, &layout, true, false);
        assert_eq!(s.caret, 2);
    }

    // ---- Cmd+←/→ visual line-edge contract (macOS A2) ----
    //
    // The handler routes `Cmd+←` / `Cmd+→` through `move_line_edge`, so these
    // tests assert the visual-line-edge behavior the user sees on macOS. They
    // run on every platform — the macOS gating is enforced by
    // `event::is_line_edge_modifier` (covered in `event::tests`).

    #[test]
    fn cmd_right_single_line_jumps_to_visual_end() {
        // First line "ab" has logical end = byte 2 (before the '\n').
        let layout = layout_abcd();
        let mut s = state_with("ab\ncd", 0);
        move_line_edge(&mut s, &layout, true, false);
        assert_eq!(s.caret, 2);
    }

    #[test]
    fn cmd_left_single_line_jumps_to_visual_start() {
        let layout = layout_abcd();
        // Caret mid-line0 (byte 1); Cmd+← → byte 0.
        let mut s = state_with("ab\ncd", 1);
        move_line_edge(&mut s, &layout, false, false);
        assert_eq!(s.caret, 0);
    }

    #[test]
    fn cmd_right_wrapped_line_clamps_no_cross() {
        // On line0, Cmd+→ lands at line0's visual end (byte 2). A second
        // Cmd+→ from the edge is a no-op (clamp) — does NOT cross to line1.
        // Mirrors macOS TextEdit; locked per brainstorm.
        let layout = layout_abcd();
        let mut s = state_with("ab\ncd", 1);
        move_line_edge(&mut s, &layout, true, false);
        assert_eq!(s.caret, 2, "first Cmd+→ lands at line0 end");
        move_line_edge(&mut s, &layout, true, false);
        assert_eq!(s.caret, 2, "second Cmd+→ at edge clamps — no cross");
    }

    #[test]
    fn cmd_left_wrapped_line_clamps_no_cross() {
        let layout = layout_abcd();
        // Caret at line1 mid (byte 4); Cmd+← lands at line1 start (byte 3).
        // Repeated Cmd+← clamps — does NOT cross to line0.
        let mut s = state_with("ab\ncd", 4);
        move_line_edge(&mut s, &layout, false, false);
        assert_eq!(s.caret, 3);
        move_line_edge(&mut s, &layout, false, false);
        assert_eq!(s.caret, 3, "repeated Cmd+← at start clamps");
    }

    #[test]
    fn cmd_right_mixed_bidi_lands_at_rightmost_visual_column() {
        // line0 "abאב" mixed: visual rightmost stop = byte 2 (logical RTL
        // start), NOT the logical line end (byte 6). End / Cmd+→ contract.
        let layout = layout_mixed_then_ltr();
        let mut s = state_with("abאב\ncd", 1);
        move_line_edge(&mut s, &layout, true, false);
        assert_eq!(s.caret, 2);
    }

    #[test]
    fn cmd_left_mixed_bidi_lands_at_leftmost_visual_column() {
        let layout = layout_mixed_then_ltr();
        let mut s = state_with("abאב\ncd", 4);
        move_line_edge(&mut s, &layout, false, false);
        assert_eq!(s.caret, 0);
    }

    #[test]
    fn shift_cmd_arrow_extends_selection() {
        let layout = layout_abcd();
        let mut s = state_with("ab\ncd", 1);
        move_line_edge(&mut s, &layout, true, true);
        assert_eq!(s.selection_anchor, Some(1), "anchor at pre-move caret");
        assert_eq!(s.caret, 2);
    }

    #[test]
    fn cmd_arrow_no_layout_lines_is_noop() {
        // Pre-first-layout: `lines.is_empty()` short-circuits in
        // `move_line_edge`. No panic; caret unchanged.
        let empty = MultilineLayout {
            lines: Vec::new(),
            total_height_lpx: 0.0,
            line_height_lpx: 0.0,
        };
        let mut s = state_with("ab", 1);
        move_line_edge(&mut s, &empty, true, false);
        assert_eq!(s.caret, 1);
    }

    #[test]
    fn enter_inserts_newline_at_caret() {
        let mut s = state_with("ab", 1);
        let text = insert_newline(&mut s);
        assert_eq!(text, "a\nb");
        assert_eq!(s.caret, 2, "caret advances past the inserted '\\n'");
        assert_eq!(s.desired_x, None);
    }

    #[test]
    fn enter_replaces_selection() {
        let mut s = state_with("abcd", 3);
        s.selection_anchor = Some(1); // select "bc"
        let text = insert_newline(&mut s);
        assert_eq!(text, "a\nd");
        assert_eq!(s.caret, 2);
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn visual_cross_line_right_enters_next_line_visual_left() {
        // Caret at line0's visual-right edge (byte 2, the RTL logical start).
        // Right → next line's visual-left edge = its byte_start (7), not a
        // logical grapheme step that would land mid-line.
        let layout = layout_mixed_then_ltr();
        let mut s = state_with("abאב\ncd", 2);
        s.caret_affinity = Affinity::Downstream;
        visual_cross_line(&mut s, &layout, true, false);
        assert_eq!(s.caret, 7);
        assert_eq!(s.caret_affinity, Affinity::Downstream);
        assert_eq!(s.selection_anchor, None);
    }

    #[test]
    fn visual_cross_line_left_enters_prev_line_visual_right() {
        // From line1 start, Left → previous (mixed) line's visual-right edge,
        // which is its logical RTL start, byte 2 (not the logical line end 6).
        let layout = layout_mixed_then_ltr();
        let mut s = state_with("abאב\ncd", 7);
        visual_cross_line(&mut s, &layout, false, false);
        assert_eq!(s.caret, 2, "enters mixed line at its visual-right edge");
    }

    #[test]
    fn visual_cross_line_clamps_at_document_edges() {
        let layout = layout_mixed_then_ltr();
        // Right on the last line → clamp (no move).
        let mut s = state_with("abאב\ncd", 9);
        visual_cross_line(&mut s, &layout, true, false);
        assert_eq!(s.caret, 9);
        // Left on the first line → clamp.
        let mut s = state_with("abאב\ncd", 2);
        visual_cross_line(&mut s, &layout, false, false);
        assert_eq!(s.caret, 2);
    }

    #[test]
    fn visual_cross_line_shift_extends_from_existing_anchor() {
        // Mirrors the handler: `apply_visual_motion` sets the anchor before
        // reporting the visual edge, so the cross-line move must extend it.
        let layout = layout_mixed_then_ltr();
        let mut s = state_with("abאב\ncd", 2);
        s.selection_anchor = Some(2); // anchor set by apply_visual_motion
        visual_cross_line(&mut s, &layout, true, true);
        assert_eq!(s.selection_anchor, Some(2), "anchor preserved");
        assert_eq!(s.caret, 7);
    }
}
