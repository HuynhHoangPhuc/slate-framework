//! Unified profiling-counter tests exercising the
//! `slate_framework::profiling` re-export surface.
//!
//! Counters are process-global atomics, so a static `Mutex` serializes the
//! tests within this binary even when cargo runs them in parallel. The
//! `CountingAllocator` is installed as `#[global_allocator]` for this binary
//! so the allocation counters are non-zero on `Box::new`.

#![cfg(feature = "profiling")]

use std::sync::{Mutex, MutexGuard, OnceLock};

use slate_framework::profiling::{self, CountingAllocator};
use slate_framework::reactive::Signal;
use slate_framework::{AnyElement, Div, HeadlessApp};

#[global_allocator]
static ALLOC: CountingAllocator = CountingAllocator;

fn lock() -> MutexGuard<'static, ()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    let m = M.get_or_init(|| Mutex::new(()));
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[test]
fn signal_set_increments_unified_signal_notify_count() {
    let _g = lock();
    profiling::reset_counters();

    let app = HeadlessApp::new(64, 64).expect("headless app");
    let runtime = app.runtime();

    let observer = runtime.next_observer_id();
    let signal: Signal<i32> = Signal::new(runtime.clone(), 0);

    slate_reactive::with_observer(observer, || {
        let _ = signal.get();
    });

    signal.set(7);

    assert_eq!(profiling::signal_notify_count(), 1);
}

#[test]
fn box_alloc_increments_alloc_count() {
    let _g = lock();
    profiling::reset_counters();

    let before = profiling::alloc_count();
    let b = Box::new(0u8);
    let after = profiling::alloc_count();

    assert!(
        after > before,
        "alloc_count did not advance: before={before}, after={after}",
    );

    drop(b);
    assert!(profiling::dealloc_count() >= 1);
}

#[test]
fn headless_render_advances_paint_cmd_count() {
    let _g = lock();
    profiling::reset_counters();

    let mut app = HeadlessApp::new(128, 128).expect("headless app");

    let before = profiling::paint_cmd_count();

    let div = Div::new().background([0.5, 0.5, 0.5, 1.0]);
    let _img = app.render(AnyElement::new(div)).expect("render");

    let after = profiling::paint_cmd_count();
    assert!(
        after > before,
        "paint_cmd_count did not advance across a Div render: before={before}, after={after}",
    );
}

#[test]
fn reset_counters_zeroes_unified_surface() {
    let _g = lock();

    // Touch each counter so they're non-zero.
    let _b = Box::new([0u8; 16]);

    let app = HeadlessApp::new(64, 64).expect("headless app");
    let runtime = app.runtime();
    let observer = runtime.next_observer_id();
    let signal: Signal<i32> = Signal::new(runtime, 0);
    slate_reactive::with_observer(observer, || {
        let _ = signal.get();
    });
    signal.set(1);

    profiling::reset_counters();

    assert_eq!(profiling::signal_notify_count(), 0);
    assert_eq!(profiling::effect_reentry_count(), 0);
    assert_eq!(profiling::paint_cmd_count(), 0);
    assert_eq!(profiling::alloc_count(), 0);
    assert_eq!(profiling::dealloc_count(), 0);
}
