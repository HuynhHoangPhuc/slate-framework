//! Per-redraw-phase counters and ns accumulators.
//!
//! Available only when the `profiling` feature is enabled. Counts one
//! `View::render` invocation and accumulates elapsed nanoseconds across the
//! four redraw phases (view-render, compute-layout, paint, present). Pairs
//! with the reactive / renderer / allocator counters re-exported from
//! [`crate::profiling`] so a bench can read the full 11-counter surface in
//! one `eprintln!`.
//!
//! Increments + `Instant::now()` calls sit behind `#[cfg(feature =
//! "profiling")]` at every call site in `app_state/render/redraw.rs`, so the
//! default build remains byte-equivalent.
//!
//! NOTE: `view_render_count` assumes a single `with_observer` site in
//! `redraw.rs` (the `View::render` block around lines 62-70). If
//! `redraw.rs` ever grows a second render site, re-audit and add a
//! `record_view_render` call there too.
//!
//! Atomic ordering matches the existing reactive counters
//! (`slate-reactive/src/profiling.rs`): `Relaxed` on every load / store /
//! fetch_add.

use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) static VIEW_RENDER_COUNT: AtomicU64 = AtomicU64::new(0);
pub(crate) static VIEW_RENDER_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static COMPUTE_LAYOUT_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PAINT_NS: AtomicU64 = AtomicU64::new(0);
pub(crate) static PRESENT_NS: AtomicU64 = AtomicU64::new(0);

/// Returns the cumulative count of `View::render` invocations since the last
/// `reset_counters` call.
pub fn view_render_count() -> u64 {
    VIEW_RENDER_COUNT.load(Ordering::Relaxed)
}

/// Returns the cumulative nanoseconds spent in the view-render phase since
/// the last `reset_counters` call.
pub fn view_render_ns() -> u64 {
    VIEW_RENDER_NS.load(Ordering::Relaxed)
}

/// Returns the cumulative nanoseconds spent in the layout-computation phase
/// since the last `reset_counters` call.
pub fn compute_layout_ns() -> u64 {
    COMPUTE_LAYOUT_NS.load(Ordering::Relaxed)
}

/// Returns the cumulative nanoseconds spent in the paint phase (including
/// focus-ring overlay emission) since the last `reset_counters` call.
pub fn paint_ns() -> u64 {
    PAINT_NS.load(Ordering::Relaxed)
}

/// Returns the cumulative nanoseconds spent in the present / render-to-window
/// phase since the last `reset_counters` call.
pub fn present_ns() -> u64 {
    PRESENT_NS.load(Ordering::Relaxed)
}

/// Resets all per-phase redraw counters to zero.
pub fn reset_counters() {
    VIEW_RENDER_COUNT.store(0, Ordering::Relaxed);
    VIEW_RENDER_NS.store(0, Ordering::Relaxed);
    COMPUTE_LAYOUT_NS.store(0, Ordering::Relaxed);
    PAINT_NS.store(0, Ordering::Relaxed);
    PRESENT_NS.store(0, Ordering::Relaxed);
}

#[inline]
pub(crate) fn record_view_render(d: std::time::Duration) {
    VIEW_RENDER_COUNT.fetch_add(1, Ordering::Relaxed);
    VIEW_RENDER_NS.fetch_add(d.as_nanos() as u64, Ordering::Relaxed);
}

#[inline]
pub(crate) fn record_layout(d: std::time::Duration) {
    COMPUTE_LAYOUT_NS.fetch_add(d.as_nanos() as u64, Ordering::Relaxed);
}

#[inline]
pub(crate) fn record_paint(d: std::time::Duration) {
    PAINT_NS.fetch_add(d.as_nanos() as u64, Ordering::Relaxed);
}

#[inline]
pub(crate) fn record_present(d: std::time::Duration) {
    PRESENT_NS.fetch_add(d.as_nanos() as u64, Ordering::Relaxed);
}
