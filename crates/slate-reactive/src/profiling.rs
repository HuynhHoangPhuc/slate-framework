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
//! - `reentrancy_count`: incremented once per `Signal::set` / `Signal::update`
//!   invoked while at least one `Effect::run` is on the same thread's call
//!   stack. Quantifies the "write-during-dispatch" pattern that the runtime's
//!   drain queue absorbs — informs the NX-22 effect-queueing verdict.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) static SIGNAL_NOTIFY_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static EFFECT_REENTRY_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static REENTRANCY_COUNT: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static EFFECT_RUN_DEPTH: Cell<u32> = const { Cell::new(0) };
}

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

/// Returns the cumulative count of `Signal::set` / `Signal::update` calls
/// observed while an `Effect::run` was on the current thread's call stack
/// since the last `reset_counters` call.
///
/// A non-zero count is expected on workloads where effects write to signals;
/// the drain queue ensures these writes are deferred, but the counter
/// surfaces how often the pattern occurs and is the canonical input to the
/// NX-22 (effect-queueing) coverage verdict in Phase 9.
pub fn reentrancy_count() -> u64 {
    REENTRANCY_COUNT.load(Ordering::Relaxed)
}

/// Resets all reactive profiling counters to zero.
pub fn reset_counters() {
    SIGNAL_NOTIFY_COUNT.store(0, Ordering::Relaxed);
    EFFECT_REENTRY_COUNT.store(0, Ordering::Relaxed);
    REENTRANCY_COUNT.store(0, Ordering::Relaxed);
}

/// Bumps `REENTRANCY_COUNT` iff an `Effect::run` is currently on the call
/// stack of the active thread. Called once per `Signal::set` /
/// `Signal::update` from the gated hook in `signal.rs`.
#[inline]
pub(crate) fn note_signal_set_for_reentry() {
    EFFECT_RUN_DEPTH.with(|d| {
        if d.get() > 0 {
            REENTRANCY_COUNT.fetch_add(1, Ordering::Relaxed);
        }
    });
}

/// RAII guard installed at the top of `EffectInner::run`. Tracks per-thread
/// effect-run nesting depth and bumps `EFFECT_REENTRY_COUNT` on every
/// synchronous re-entry (depth >= 2 at enter time).
pub(crate) struct ReentryGuard;

impl ReentryGuard {
    pub(crate) fn enter() -> Self {
        EFFECT_RUN_DEPTH.with(|d| {
            let prev = d.get();
            if prev > 0 {
                EFFECT_REENTRY_COUNT.fetch_add(1, Ordering::Relaxed);
            }
            d.set(prev + 1);
        });
        Self
    }
}

impl Drop for ReentryGuard {
    fn drop(&mut self) {
        EFFECT_RUN_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
    }
}
