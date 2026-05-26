//! Profiling counter for the renderer paint path.
//!
//! Available only when the `profiling` feature is enabled. The increment
//! helper is `#[inline]` and gated by `#[cfg(feature = "profiling")]` at
//! every call site, so the default build keeps a byte-equivalent paint path.
//!
//! # Counter
//!
//! - `paint_cmd_count`: incremented once per non-empty `pass.draw` emission
//!   across the four instanced pipelines (rect / glyph / image / shadow).
//!   Empty ranges short-circuit before the counter fires, so the value
//!   tracks executed draw commands, not enqueued ones.

use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) static PAINT_CMD_COUNT: AtomicU64 = AtomicU64::new(0);

/// Returns the cumulative paint-command count since the last
/// `reset_counters` call.
pub fn paint_cmd_count() -> u64 {
    PAINT_CMD_COUNT.load(Ordering::Relaxed)
}

/// Resets the paint-command counter to zero.
pub fn reset_counters() {
    PAINT_CMD_COUNT.store(0, Ordering::Relaxed);
}
