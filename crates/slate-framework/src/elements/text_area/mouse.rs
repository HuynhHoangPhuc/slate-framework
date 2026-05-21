//! TextArea pointer handlers: click-to-place caret + drag-select across lines.
//!
//! Mirrors the single-line `text_field` mouse handlers but resolves the click
//! against the cached [`MultilineLayout`](slate_text::MultilineLayout) via
//! [`byte_at_point`], so a click lands on the correct visual line + column and
//! a drag extends a selection that may span lines. The layout + paint origin
//! are written onto `ImeState` by `paint` on the previous frame; the
//! first-frame race (no cached layout yet) resolves itself on the next paint.
//!
//! IME guard: pointer interaction is a no-op during an active preedit
//! composition (platforms expect `selectedRange == markedRange` while
//! composing), matching TextField.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::elements::text_edit::ops::reset_blink;
use crate::elements::text_edit::word::word_range_at;
use crate::event::{EventCtx, MouseEvent, MouseHandler};

use super::layout::byte_at_point;

/// Maximum gap between two mouse-downs that still counts as one multi-click run.
/// 500ms matches the common platform default (Windows `GetDoubleClickTime`
/// defaults to 500ms; macOS is configurable around the same range).
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

/// Maximum pointer travel (logical px) between clicks of one run. A click that
/// lands farther than this from the previous one starts a fresh run, so a slow
/// drag-then-click never reads as a double click.
const DOUBLE_CLICK_DISTANCE: f32 = 4.0;

/// Advance the multi-click run counter. Pure so the timing/distance state
/// machine is unit-testable without a live pointer.
///
/// A click continues the run (1→2→3) only when it lands within both
/// [`DOUBLE_CLICK_INTERVAL`] and [`DOUBLE_CLICK_DISTANCE`] of the previous one.
/// A triple click wraps back to 1 (a fourth click re-places the caret), and any
/// out-of-window click — including the first ever (`prev_time == None`) — resets
/// to 1.
fn next_click_count(
    prev_time: Option<Instant>,
    prev_pos: (f32, f32),
    prev_count: u8,
    now: Instant,
    pos: (f32, f32),
) -> u8 {
    let in_time = prev_time.is_some_and(|t| now.duration_since(t) <= DOUBLE_CLICK_INTERVAL);
    let dx = pos.0 - prev_pos.0;
    let dy = pos.1 - prev_pos.1;
    let in_dist = dx * dx + dy * dy <= DOUBLE_CLICK_DISTANCE * DOUBLE_CLICK_DISTANCE;
    if in_time && in_dist {
        match prev_count {
            1 => 2,
            2 => 3,
            _ => 1, // triple → wrap; 0/unknown → fresh single
        }
    } else {
        1
    }
}

/// Build the `on_mouse_down` handler. Resolves the clicked byte, advances the
/// multi-click counter, then sets caret + selection anchor for a single click
/// (collapsed caret), double click (the Unicode word under the pointer), or
/// triple click (the whole visual line). Always begins a drag and clears the
/// sticky vertical-nav column.
pub(super) fn build_mouse_down_handler() -> MouseHandler {
    Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
        let Some(id) = cx.element_id() else { return };
        let Some(state_rc) = cx.ime_state(id) else {
            return;
        };

        {
            let mut state = state_rc.borrow_mut();
            if state.preedit.is_some() {
                return;
            }
            let Some(layout) = state.last_layout.clone() else {
                return;
            };
            let byte = byte_at_point(
                &layout,
                &state.text,
                state.paint_origin_x,
                state.paint_origin_y,
                ev.position.0,
                ev.position.1,
            );
            debug_assert!(
                state.text.is_char_boundary(byte),
                "byte_at_point must return a char boundary"
            );

            let count = next_click_count(
                state.last_click_time,
                state.last_click_pos,
                state.click_count,
                ev.timestamp,
                ev.position,
            );
            state.last_click_time = Some(ev.timestamp);
            state.last_click_pos = ev.position;
            state.click_count = count;

            // Pick the (anchor, caret) span for this click level. Word/line
            // ranges read `state.text`/`layout` immutably before the mutation.
            let (anchor, caret) = match count {
                2 => {
                    let r = word_range_at(&state.text, byte);
                    (r.start, r.end)
                }
                3 => match layout.lines.get(layout.line_for_byte(byte)) {
                    // Whole visual line including its terminator (the soft-wrap
                    // boundary or trailing '\n' folded into `byte_end`), so a
                    // triple-click + delete removes the line break too.
                    Some(l) => (l.byte_start, l.byte_end),
                    None => (byte, byte),
                },
                _ => (byte, byte),
            };

            state.caret = caret;
            state.caret_affinity = slate_text::Affinity::Downstream;
            state.selection_anchor = Some(anchor);
            state.dragging = true;
            state.desired_x = None;
            reset_blink(&mut state);
            state.undo.mark_motion();
        }
        cx.stop_propagation();
    })
}

/// Build the `on_mouse_move` handler: while dragging (and not composing),
/// extend the caret to the pointer; the anchor stays put so the selection
/// grows. Inert outside a drag.
pub(super) fn build_mouse_move_handler() -> MouseHandler {
    Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
        let Some(id) = cx.element_id() else { return };
        let Some(state_rc) = cx.ime_state(id) else {
            return;
        };

        let mut state = state_rc.borrow_mut();
        if !state.dragging || state.preedit.is_some() {
            return;
        }
        let Some(layout) = state.last_layout.clone() else {
            return;
        };
        let byte = byte_at_point(
            &layout,
            &state.text,
            state.paint_origin_x,
            state.paint_origin_y,
            ev.position.0,
            ev.position.1,
        );
        debug_assert!(
            state.text.is_char_boundary(byte),
            "byte_at_point must return a char boundary"
        );
        state.caret = byte;
        state.caret_affinity = slate_text::Affinity::Downstream;
        reset_blink(&mut state);
        drop(state);
        cx.stop_propagation();
    })
}

/// Build the `on_mouse_up` handler: end the drag and collapse the selection
/// when the anchor and caret coincide (a plain click). Always runs, even during
/// preedit, so a `dragging` flag set before composition started can be cleared.
pub(super) fn build_mouse_up_handler() -> MouseHandler {
    Arc::new(move |_ev: &MouseEvent, cx: &mut EventCtx| {
        let Some(id) = cx.element_id() else { return };
        let Some(state_rc) = cx.ime_state(id) else {
            return;
        };

        {
            let mut state = state_rc.borrow_mut();
            state.dragging = false;
            if state.selection_anchor == Some(state.caret) {
                state.selection_anchor = None;
            }
        }
        cx.stop_propagation();
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const POS: (f32, f32) = (50.0, 20.0);

    #[test]
    fn first_click_is_single() {
        // No prior click (prev_time None) → always a fresh single click.
        assert_eq!(next_click_count(None, (0.0, 0.0), 0, Instant::now(), POS), 1);
    }

    #[test]
    fn two_close_clicks_make_a_double_then_triple() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(100);
        let t2 = t1 + Duration::from_millis(100);
        // 1 → 2 (double), 2 → 3 (triple), within time + distance.
        assert_eq!(next_click_count(Some(t0), POS, 1, t1, POS), 2);
        assert_eq!(next_click_count(Some(t1), POS, 2, t2, POS), 3);
    }

    #[test]
    fn fourth_click_wraps_to_single() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(100);
        // After a triple (count 3), the next in-window click restarts at 1.
        assert_eq!(next_click_count(Some(t0), POS, 3, t1, POS), 1);
    }

    #[test]
    fn slow_second_click_resets_to_single() {
        let t0 = Instant::now();
        let t_slow = t0 + Duration::from_millis(600); // > 500ms interval
        assert_eq!(next_click_count(Some(t0), POS, 1, t_slow, POS), 1);
    }

    #[test]
    fn far_second_click_resets_to_single() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(100);
        let far = (POS.0 + 10.0, POS.1); // > 4px away
        assert_eq!(next_click_count(Some(t0), POS, 1, t1, far), 1);
    }

    #[test]
    fn small_jitter_within_tolerance_still_doubles() {
        let t0 = Instant::now();
        let t1 = t0 + Duration::from_millis(100);
        // 3px of jitter (3² = 9 ≤ 16 = 4²) is inside the tolerance → still a double.
        let jittered = (POS.0 + 3.0, POS.1);
        assert_eq!(next_click_count(Some(t0), POS, 1, t1, jittered), 2);
    }
}
