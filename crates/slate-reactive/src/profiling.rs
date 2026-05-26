//! Profiling counters for reactive primitives.
//!
//! Available only when the `profiling` feature is enabled. Counters compile
//! out of shipped binaries — the increment helpers are `#[inline]` and
//! gated by `#[cfg(feature = "profiling")]` at every call site.
//!
//! # Counters
//!
//! - `signal_notify_count`: incremented once per observer dispatched by
//!   `Signal::set` / `Signal::update` (i.e. per subscriber, not per `set` call).
//! - `effect_reentry_count`: incremented when `Effect::run` is invoked while
//!   another effect is already running on the same thread. A non-zero count
//!   surfaces a synchronous re-entry pattern that can mask cascading writes.

use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) static SIGNAL_NOTIFY_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static EFFECT_REENTRY_COUNT: AtomicU64 = AtomicU64::new(0);

/// Returns the cumulative count of observer dispatches triggered by signal
/// updates since the last `reset_counters` call.
pub fn signal_notify_count() -> u64 {
    SIGNAL_NOTIFY_COUNT.load(Ordering::Relaxed)
}

/// Returns the cumulative count of synchronous effect re-entries since the
/// last `reset_counters` call.
pub fn effect_reentry_count() -> u64 {
    EFFECT_REENTRY_COUNT.load(Ordering::Relaxed)
}

/// Resets all reactive profiling counters to zero.
pub fn reset_counters() {
    SIGNAL_NOTIFY_COUNT.store(0, Ordering::Relaxed);
    EFFECT_REENTRY_COUNT.store(0, Ordering::Relaxed);
}
