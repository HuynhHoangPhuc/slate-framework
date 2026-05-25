//! Recovery state machine types + tunable constants shared across
//! `app_state/render/*` and `dispatch_redraw`.
//!
//! Borrow-order discipline (ADR-001) is enforced via `guards::reset_borrow_order`;
//! see `guards.rs` for the runtime check.

use std::time::Instant;

// Recovery state machine constants.
// Test-fast overrides (test-hooks feature) shrink each cycle to ~tens of ms so
// integration tests can exercise multiple recovery cycles inside a 5-second
// wall-clock budget without burning CPU on cooldown sleeps.
#[cfg(not(feature = "test-hooks"))]
pub(crate) const RECOVERY_COOLDOWN_MS: u64 = 350;
#[cfg(feature = "test-hooks")]
pub(crate) const RECOVERY_COOLDOWN_MS: u64 = 20;

pub(crate) const RECOVERY_MAX_ATTEMPTS: u32 = 5;

#[cfg(not(feature = "test-hooks"))]
pub(crate) const RECOVERY_BACKOFF_BASE_MS: u64 = 100;
#[cfg(feature = "test-hooks")]
pub(crate) const RECOVERY_BACKOFF_BASE_MS: u64 = 10;

pub(crate) const RECOVERY_BACKOFF_STEP_MS: u64 = 10;
pub(crate) const RECOVERY_FLAP_GUARD_SECS: u64 = 5;

// Minimum spacing between adapter-LUID probes in `dispatch_redraw`.
// During a cross-monitor drag the window can cross the boundary many times in
// quick succession; probing every redraw would mark device-lost repeatedly and
// thrash the recovery state machine. 100ms is short enough to feel instant on
// a single deliberate drag yet long enough to absorb the natural drag-jitter
// burst (~60fps × 1–2 frames straddling the seam).
pub(crate) const ADAPTER_PROBE_MIN_INTERVAL_MS: u64 = 100;

/// Origin classification for a device-lost event.
///
/// Distinguishes user-initiated cross-adapter migrations (`LuidMigration` —
/// the per-redraw LUID probe noticed the window moved to a monitor served by
/// a different adapter and synthetically marked the device lost) from real
/// driver/TDR faults reported by wgpu's `set_device_lost_callback`
/// (`WgpuCallback`).
///
/// Only `WgpuCallback` contributes to the 5-second flap guard. `LuidMigration`
/// bypasses it — repeated cross-seam drags are healthy, not a fault loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceLossReason {
    /// Window moved across adapters; recovery rebuilds on the new adapter.
    LuidMigration,
    /// wgpu lost-callback fired — real driver reset, TDR, or device removal.
    WgpuCallback,
}

/// Recovery state machine for device-lost handling.
///
/// Replaces the old 3-shot immediate retry with a zed-validated pattern:
/// 350ms cooldown, 5-attempt backoff, and skip_draws gating.
///
/// Each non-terminal active variant carries the `DeviceLossReason` so the
/// flap-guard predicate can apply reason-aware semantics.
#[derive(Debug, Clone)]
pub enum RecoveryState {
    /// Device is healthy, no recovery in progress.
    NotLost,
    /// Device loss just detected; waiting to transition to cooldown.
    DetectedLost {
        detected_at: Instant,
        reason: DeviceLossReason,
    },
    /// Loss occurred during the WM_ENTERSIZEMOVE..WM_EXITSIZEMOVE modal loop.
    /// Recovery is deferred until the modal loop exits, so a
    /// drag that crosses adapters multiple times in a single gesture collapses
    /// into one recovery cycle. Exits to `CooldownGate` on `on_size_move_end`.
    DeferredUntilStable {
        detected_at: Instant,
        reason: DeviceLossReason,
    },
    /// Waiting for 350ms cooldown before retry attempts.
    CooldownGate {
        since: Instant,
        reason: DeviceLossReason,
    },
    /// Actively retrying device recreation.
    Retrying {
        attempt: u32,
        last_attempt_at: Instant,
        reason: DeviceLossReason,
    },
    /// Recovery succeeded; will transition to NotLost on next redraw.
    Recovered { at: Instant },
    /// Recovery exhausted all attempts; app should quit.
    GiveUp { reason: DeviceLossReason },
}

/// Signal returned by dispatch methods to communicate with the event loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppSignal {
    None,
    RequestQuit,
    RequestRedraw,
}
