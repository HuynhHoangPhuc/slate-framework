//! Unified profiling counter API.
//!
//! Available only when the `profiling` feature is enabled. Re-exports the
//! per-crate counters from [`slate_reactive::profiling`] and
//! [`slate_renderer::profiling`], and owns the optional counting allocator
//! (`counting_alloc`).
//!
//! The default build is byte-equivalent — every counter increment sits
//! behind a `#[cfg(feature = "profiling")]` gate at the call site.
//!
//! # Counters
//!
//! - `signal_notify_count` — observer dispatches from `Signal::set`.
//! - `effect_reentry_count` — synchronous effect re-entries.
//! - `reentrancy_count` — `Signal::set` calls observed while an `Effect::run`
//!   is on the same thread's call stack (write-during-dispatch pattern).
//! - `paint_cmd_count` — non-empty `pass.draw` emissions across the four
//!   instanced pipelines.
//! - `alloc_count` / `dealloc_count` — heap activity, when
//!   [`CountingAllocator`] is installed as `#[global_allocator]`.
//! - `view_render_count` — `View::render` invocations from the redraw pipeline.
//! - `view_render_ns` / `compute_layout_ns` / `paint_ns` / `present_ns` —
//!   cumulative nanoseconds across the four redraw phases.
//!
//! # Reset semantics
//!
//! Bench harnesses call [`reset_counters`] at warmup-end and read deltas per
//! iteration. Counters are process-global atomics — concurrent reads from
//! multiple threads are safe but the values reflect process-wide activity.

pub mod counting_alloc;
pub mod redraw_counters;

pub use counting_alloc::{
    CountingAllocator, alloc_count, dealloc_count, reset_counters as reset_alloc_counters,
};
pub use redraw_counters::{
    compute_layout_ns, paint_ns, present_ns, view_render_count, view_render_ns,
};
pub use slate_reactive::profiling::{
    effect_reentry_count, reentrancy_count, reset_counters as reset_reactive_counters,
    signal_notify_count,
};
pub use slate_renderer::profiling::{paint_cmd_count, reset_counters as reset_renderer_counters};

/// Resets every profiling counter (reactive, renderer, allocator, redraw) to zero.
pub fn reset_counters() {
    reset_reactive_counters();
    reset_renderer_counters();
    reset_alloc_counters();
    redraw_counters::reset_counters();
}
