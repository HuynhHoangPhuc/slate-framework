//! Reference-scene drivers for the slate-framework benchmark suite.
//!
//! Each scene exposes a `build()` constructor returning a `View` that can be
//! driven through `HeadlessApp::render_view` in a tight loop. Scenes are
//! intentionally small ports of the canonical example apps — they live here
//! (not under `examples/`) so criterion benches and the smoke test can render
//! them without standing up a real window.
//!
//! # Counter snapshots
//!
//! Benches read `slate_framework::profiling::{paint_cmd_count,
//! signal_notify_count, effect_reentry_count, alloc_count}` after a measured
//! batch and `eprintln!` the delta. The default build is byte-equivalent —
//! counter increments compile out — so scenes must be built with the
//! `profiling` feature for the numbers to be non-zero.

pub mod scenes;

use slate_framework::HeadlessApp;

/// Standard bench viewport: 1024x768, scale 1.0.
pub const BENCH_WIDTH: u32 = 1024;
pub const BENCH_HEIGHT: u32 = 768;

/// Construct a `HeadlessApp` sized for the bench suite. Panics on GPU init
/// failure — bench harnesses cannot proceed without a working device.
pub fn headless_app() -> HeadlessApp {
    HeadlessApp::new(BENCH_WIDTH, BENCH_HEIGHT)
        .expect("HeadlessApp init failed — no GPU adapter available?")
}
