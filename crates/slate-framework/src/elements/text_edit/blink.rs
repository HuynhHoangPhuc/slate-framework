//! Caret blink advance, shared by `TextField` and `TextArea`.
//!
//! Paint calls [`advance_blink`] once per frame. While focused it toggles
//! visibility on a 530 ms half-period (matching macOS
//! `NSTextInsertionPointBlinkPeriod`) and returns the next deadline so paint
//! can schedule a redraw. While unfocused it resets the cycle (visible, no
//! armed deadline) so the next focus gain starts fresh — and returns no
//! deadline, so an unfocused field schedules nothing (zero CPU).

use std::time::{Duration, Instant};

use crate::ime::BlinkState;

/// Caret blink half-period. Matches macOS `NSTextInsertionPointBlinkPeriod`.
pub(crate) const CARET_BLINK_PERIOD: Duration = Duration::from_millis(530);

/// Advance the blink cycle for one paint frame.
///
/// Returns `(caret_visible, next_deadline)`:
/// - `caret_visible` — whether the caret should be drawn this frame.
/// - `next_deadline` — `Some(instant)` to schedule a redraw at (focused only);
///   `None` when unfocused (nothing to schedule).
pub(crate) fn advance_blink(
    blink: &mut BlinkState,
    focused: bool,
    now: Instant,
) -> (bool, Option<Instant>) {
    if !focused {
        // Unfocused: drop any in-flight blink so the next focus gain starts a
        // fresh cycle from visible = true. Caret not drawn; nothing scheduled.
        blink.visible = true;
        blink.next = None;
        return (false, None);
    }
    match blink.next {
        None => {
            blink.visible = true;
            blink.next = Some(now + CARET_BLINK_PERIOD);
        }
        Some(t) if now >= t => {
            blink.visible = !blink.visible;
            blink.next = Some(now + CARET_BLINK_PERIOD);
        }
        Some(_) => {}
    }
    (blink.visible, blink.next)
}

// ---------------------------------------------------------------------------
// Unit tests — blink advance
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unfocused_resets_cycle_and_schedules_nothing() {
        let mut blink = BlinkState {
            visible: false,
            next: Some(Instant::now()),
        };
        let (visible, next) = advance_blink(&mut blink, false, Instant::now());
        assert!(!visible, "caret not drawn while unfocused");
        assert_eq!(next, None, "nothing scheduled while unfocused");
        assert!(blink.visible, "cycle reset so refocus starts visible");
        assert_eq!(blink.next, None);
    }

    #[test]
    fn focused_first_call_arms_deadline_and_is_visible() {
        let mut blink = BlinkState {
            visible: true,
            next: None,
        };
        let now = Instant::now();
        let (visible, next) = advance_blink(&mut blink, true, now);
        assert!(visible);
        assert_eq!(next, Some(now + CARET_BLINK_PERIOD));
    }

    #[test]
    fn focused_past_deadline_toggles_visibility() {
        let now = Instant::now();
        let mut blink = BlinkState {
            visible: true,
            next: Some(now - Duration::from_millis(1)),
        };
        let (visible, next) = advance_blink(&mut blink, true, now);
        assert!(!visible, "deadline passed → toggled off");
        assert_eq!(next, Some(now + CARET_BLINK_PERIOD), "next deadline re-armed");
    }

    #[test]
    fn focused_before_deadline_holds_state() {
        let now = Instant::now();
        let deadline = now + CARET_BLINK_PERIOD;
        let mut blink = BlinkState {
            visible: true,
            next: Some(deadline),
        };
        let (visible, next) = advance_blink(&mut blink, true, now);
        assert!(visible, "before deadline → unchanged");
        assert_eq!(next, Some(deadline), "deadline unchanged");
    }
}
