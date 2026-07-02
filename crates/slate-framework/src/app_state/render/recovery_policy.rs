//! Pure wait-until-healthy recovery-policy decisions.
//!
//! Isolated from the borrow-heavy state-machine mechanics in `recovery.rs` /
//! `dispatch.rs` so the *policy* (how long to keep probing, how to space
//! attempts, when to finally give up) can be unit-tested with no platform
//! window, renderer, or DXGI surface. The integration harnesses always run
//! against a *healthy* adapter, so they can only exercise the happy recovery
//! path; the persistent-failure and spacing decisions live here where they can
//! be driven with explicit `Duration`s.

use std::time::Duration;

use super::super::types::{
    RECOVERY_BACKOFF_BASE_MS, RECOVERY_BACKOFF_MAX_MS, RECOVERY_BACKOFF_STEP_MS,
    RECOVERY_GIVEUP_WALL_MS,
};

/// Minimum spacing before the next rebuild-as-probe attempt.
///
/// Grows linearly with the attempt count and is capped at
/// `RECOVERY_BACKOFF_MAX_MS`, so probing a still-suspended adapter never
/// tight-loops yet never stalls longer than the cap once it is back.
pub(super) fn recovery_backoff(attempt: u32) -> Duration {
    let ms = RECOVERY_BACKOFF_BASE_MS
        .saturating_add((attempt as u64).saturating_mul(RECOVERY_BACKOFF_STEP_MS))
        .min(RECOVERY_BACKOFF_MAX_MS);
    Duration::from_millis(ms)
}

/// True once a recovery episode has failed *continuously* for the give-up
/// budget. `elapsed_since_first_failure` is measured from the episode's first
/// probe; any successful rebuild resets the anchor upstream, so this fires only
/// on an unbroken failure streak — the last-resort quit.
pub(super) fn recovery_should_give_up(elapsed_since_first_failure: Duration) -> bool {
    elapsed_since_first_failure >= Duration::from_millis(RECOVERY_GIVEUP_WALL_MS)
}

/// True when enough time has elapsed since the last attempt to fire the next
/// spaced probe. Attempt 0 (the immediate first probe after cooldown) is always
/// ready; later attempts wait out the [`recovery_backoff`] interval so the
/// retry loop polls (rendering off) instead of hammering rebuilds.
pub(super) fn recovery_ready_for_next_attempt(since_last_attempt: Duration, attempt: u32) -> bool {
    attempt == 0 || since_last_attempt >= recovery_backoff(attempt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_starts_at_base_and_grows() {
        assert_eq!(
            recovery_backoff(0),
            Duration::from_millis(RECOVERY_BACKOFF_BASE_MS),
            "attempt 0 backoff must equal the base"
        );
        assert!(
            recovery_backoff(1) >= recovery_backoff(0),
            "backoff must be non-decreasing in attempt count"
        );
        assert!(
            recovery_backoff(2) >= recovery_backoff(1),
            "backoff must be non-decreasing in attempt count"
        );
    }

    #[test]
    fn backoff_is_capped_at_max() {
        // A very large attempt count must not exceed the cap (and must not
        // overflow — saturating arithmetic guards that).
        let capped = recovery_backoff(u32::MAX);
        assert_eq!(
            capped,
            Duration::from_millis(RECOVERY_BACKOFF_MAX_MS),
            "backoff must saturate at the cap, never tight-loop or overflow"
        );
    }

    #[test]
    fn does_not_give_up_below_budget() {
        assert!(
            !recovery_should_give_up(Duration::ZERO),
            "must never give up at the first failure"
        );
        assert!(
            !recovery_should_give_up(Duration::from_millis(RECOVERY_GIVEUP_WALL_MS - 1)),
            "must keep probing right up to the wall-clock budget"
        );
    }

    #[test]
    fn gives_up_only_at_or_past_budget() {
        assert!(
            recovery_should_give_up(Duration::from_millis(RECOVERY_GIVEUP_WALL_MS)),
            "must give up once continuous failure reaches the budget"
        );
        assert!(
            recovery_should_give_up(Duration::from_millis(RECOVERY_GIVEUP_WALL_MS + 1_000)),
            "must give up past the budget"
        );
    }

    #[test]
    fn first_attempt_is_always_ready() {
        assert!(
            recovery_ready_for_next_attempt(Duration::ZERO, 0),
            "attempt 0 fires immediately after cooldown"
        );
    }

    #[test]
    fn later_attempt_waits_out_backoff() {
        // Just after a failed attempt 1, the backoff interval has not elapsed.
        assert!(
            !recovery_ready_for_next_attempt(Duration::ZERO, 1),
            "attempt 1 must wait out its backoff before re-probing"
        );
        // Once the backoff interval passes, the next probe is ready.
        assert!(
            recovery_ready_for_next_attempt(recovery_backoff(1), 1),
            "attempt 1 must be ready once its backoff interval elapses"
        );
    }
}
