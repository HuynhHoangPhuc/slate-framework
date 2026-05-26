//! Profiling counter tests for `slate-reactive`.
//!
//! Counters are process-global atomics, so a static `Mutex` serializes the
//! tests within this binary even when cargo runs them in parallel.

#![cfg(feature = "profiling")]

use std::sync::{Mutex, MutexGuard, OnceLock};

use slate_reactive::profiling;
use slate_reactive::{Effect, ReactiveOwner, Runtime, Signal, with_observer};

fn lock() -> MutexGuard<'static, ()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    let m = M.get_or_init(|| Mutex::new(()));
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[test]
fn signal_set_increments_notify_count_per_subscriber() {
    let _g = lock();
    profiling::reset_counters();

    let rt = Runtime::new();
    let observer = rt.next_observer_id();
    let signal: Signal<i32> = Signal::new(rt.clone(), 0);

    with_observer(observer, || {
        let _ = signal.get();
    });

    signal.set(1);

    assert_eq!(profiling::signal_notify_count(), 1);
}

#[test]
fn signal_set_without_subscribers_does_not_increment() {
    let _g = lock();
    profiling::reset_counters();

    let rt = Runtime::new();
    let signal: Signal<i32> = Signal::new(rt, 0);
    signal.set(1);

    assert_eq!(profiling::signal_notify_count(), 0);
}

#[test]
fn signal_set_with_multiple_subscribers_increments_per_subscriber() {
    let _g = lock();
    profiling::reset_counters();

    let rt = Runtime::new();
    let signal: Signal<i32> = Signal::new(rt.clone(), 0);

    let o1 = rt.next_observer_id();
    let o2 = rt.next_observer_id();
    let o3 = rt.next_observer_id();

    with_observer(o1, || {
        let _ = signal.get();
    });
    with_observer(o2, || {
        let _ = signal.get();
    });
    with_observer(o3, || {
        let _ = signal.get();
    });

    signal.set(7);

    assert_eq!(profiling::signal_notify_count(), 3);
}

#[test]
fn reset_counters_zeroes_signal_notify_count() {
    let _g = lock();
    profiling::reset_counters();

    let rt = Runtime::new();
    let observer = rt.next_observer_id();
    let signal: Signal<i32> = Signal::new(rt.clone(), 0);

    with_observer(observer, || {
        let _ = signal.get();
    });
    signal.set(1);
    assert!(profiling::signal_notify_count() >= 1);

    profiling::reset_counters();

    assert_eq!(profiling::signal_notify_count(), 0);
    assert_eq!(profiling::effect_reentry_count(), 0);
}

#[test]
fn effect_run_without_reentry_keeps_counter_at_zero() {
    let _g = lock();
    profiling::reset_counters();

    let rt = Runtime::new();
    let owner = ReactiveOwner::root();
    let _guard = owner.enter();

    let signal = Signal::new(rt.clone(), 0i32);
    let s = signal.clone();
    let _effect = Effect::new(rt.clone(), move || {
        let _ = s.get_untracked();
    });

    signal.set(1);
    rt.drain_effects();

    assert_eq!(profiling::effect_reentry_count(), 0);
}

#[test]
fn synchronous_drain_inside_effect_records_reentry() {
    let _g = lock();
    profiling::reset_counters();

    let rt = Runtime::new();
    let owner = ReactiveOwner::root();
    let _guard = owner.enter();

    // Inner effect: subscribes to `inner_signal` so it can be queued
    // synchronously from outer's closure body.
    let inner_signal = Signal::new(rt.clone(), 0i32);
    let inner_read = inner_signal.clone();
    let _inner_effect = Effect::new(rt.clone(), move || {
        // Establishes subscription on the active observer (the effect's id).
        let _ = inner_read.get();
    });

    // Outer effect: subscribes to `trigger`. When it re-runs from drain it
    // sets `inner_signal` (queueing inner) then synchronously drains, which
    // re-enters `EffectInner::run` while outer's run is still on the stack.
    let trigger = Signal::new(rt.clone(), 0i32);
    let trigger_read = trigger.clone();
    let inner_write = inner_signal.clone();
    let rt_for_outer = rt.clone();
    let _outer_effect = Effect::new(rt.clone(), move || {
        let _ = trigger_read.get();
        inner_write.set(99);
        rt_for_outer.drain_effects();
    });

    // Reset after setup (Effect::new fires both closures once via
    // `with_observer`, not `EffectInner::run`, but inner is queued+drained
    // synchronously when outer constructs, so a setup reentry could land
    // here on first construction order. Reset wipes it.)
    profiling::reset_counters();

    // Fire the outer effect through the drain loop. Outer's run executes,
    // queues inner, then drains it synchronously → reentry.
    trigger.set(1);
    rt.drain_effects();

    assert!(
        profiling::effect_reentry_count() >= 1,
        "expected at least one reentry, got {}",
        profiling::effect_reentry_count(),
    );
}
