//! `reentrancy_count` covers `Signal::set` invoked while an `Effect::run` is
//! on the same thread's call stack. This is the "write-during-dispatch"
//! pattern the drain queue absorbs — the counter records how often it shows
//! up in real workloads (informs the NX-22 verdict in Phase 9).

#![cfg(feature = "profiling")]

use std::sync::{Mutex, MutexGuard, OnceLock};

use slate_reactive::profiling;
use slate_reactive::{Effect, ReactiveOwner, Runtime, Signal};

fn lock() -> MutexGuard<'static, ()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    let m = M.get_or_init(|| Mutex::new(()));
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[test]
fn signal_set_outside_effect_does_not_bump_reentrancy() {
    let _g = lock();
    profiling::reset_counters();

    let rt = Runtime::new();
    let signal: Signal<i32> = Signal::new(rt, 0);
    signal.set(1);

    assert_eq!(profiling::reentrancy_count(), 0);
}

#[test]
fn signal_set_inside_effect_rerun_bumps_reentrancy() {
    let _g = lock();
    profiling::reset_counters();

    let rt = Runtime::new();
    let owner = ReactiveOwner::root();
    let _guard = owner.enter();

    let trigger = Signal::new(rt.clone(), 0i32);
    let inner = Signal::new(rt.clone(), 0i32);

    let trigger_read = trigger.clone();
    let inner_write = inner.clone();
    let _effect = Effect::new(rt.clone(), move || {
        let _ = trigger_read.get();
        inner_write.set(99);
    });

    // The constructor closure executes via `with_observer` (not
    // `EffectInner::run`), so EFFECT_RUN_DEPTH is 0 during setup and the
    // reentrancy bookkeeping must not fire there. Reset wipes the
    // construction-time `signal_notify` etc. anyway.
    profiling::reset_counters();

    trigger.set(1);
    rt.drain_effects();

    assert_eq!(
        profiling::reentrancy_count(),
        1,
        "expected exactly one reentrancy (inner.set during outer effect rerun), \
         got {}",
        profiling::reentrancy_count(),
    );
}

#[test]
fn multiple_inner_sets_each_bump_reentrancy() {
    let _g = lock();
    profiling::reset_counters();

    let rt = Runtime::new();
    let owner = ReactiveOwner::root();
    let _guard = owner.enter();

    let trigger = Signal::new(rt.clone(), 0i32);
    let inner_a = Signal::new(rt.clone(), 0i32);
    let inner_b = Signal::new(rt.clone(), 0i32);
    let inner_c = Signal::new(rt.clone(), 0i32);

    let trigger_read = trigger.clone();
    let write_a = inner_a.clone();
    let write_b = inner_b.clone();
    let write_c = inner_c.clone();
    let _effect = Effect::new(rt.clone(), move || {
        let _ = trigger_read.get();
        write_a.set(1);
        write_b.set(2);
        write_c.set(3);
    });

    profiling::reset_counters();

    trigger.set(1);
    rt.drain_effects();

    assert_eq!(profiling::reentrancy_count(), 3);
}

#[test]
fn reset_counters_zeroes_reentrancy() {
    let _g = lock();
    profiling::reset_counters();

    let rt = Runtime::new();
    let owner = ReactiveOwner::root();
    let _guard = owner.enter();

    let trigger = Signal::new(rt.clone(), 0i32);
    let inner = Signal::new(rt.clone(), 0i32);
    let trigger_read = trigger.clone();
    let inner_write = inner.clone();
    let _effect = Effect::new(rt.clone(), move || {
        let _ = trigger_read.get();
        inner_write.set(99);
    });

    profiling::reset_counters();
    trigger.set(1);
    rt.drain_effects();
    assert!(profiling::reentrancy_count() >= 1);

    profiling::reset_counters();
    assert_eq!(profiling::reentrancy_count(), 0);
}
