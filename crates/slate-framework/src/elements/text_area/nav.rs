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
    apply_motion_to, delete_selection, record_edit, reset_blink,
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
    let (line_idx, x, _) = layout.caret_position(state.caret);
    // Seed the sticky column once; reuse it on every subsequent vertical press.
    let sticky = state.desired_x.unwrap_or(x);
    state.desired_x = Some(sticky);

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
    let target = if to_end {
        layout.line_caret_end(&state.text, line_idx)
    } else {
        layout.lines[line_idx].byte_start
    };
    apply_motion_to(state, shift, target);
    state.desired_x = None;
    finish_motion(state);
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
    state.text.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use slate_text::{MultilineLayout, VisualLine};
    use slate_text::{Direction, FontId, ShapedGlyph, ShapedLine};

    fn glyph(cluster: u32, adv: f32) -> ShapedGlyph {
        ShapedGlyph {
            glyph_id: 1,
            font_id: FontId::PRIMARY,
            font_handle: slate_text::FontHandle::default(),
            x_advance_lpx: adv,
            position_lpx: [0.0, 0.0],
            cluster,
            direction: Direction::Ltr,
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
}
