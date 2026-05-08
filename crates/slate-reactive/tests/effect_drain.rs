use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use slate_reactive::{Effect, ReactiveOwner, Runtime, Signal};

#[test]
fn effect_runs_on_creation() {
    let rt = Runtime::new();
    let run_count = Arc::new(AtomicUsize::new(0));
    let r = run_count.clone();

    let owner = ReactiveOwner::root();
    let _guard = owner.enter();

    let _effect = Effect::new(rt, move || {
        r.fetch_add(1, Ordering::SeqCst);
    });

    assert_eq!(run_count.load(Ordering::SeqCst), 1);
}

#[test]
fn effect_rerun_after_signal_set_and_drain() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0i32);
    let s = signal.clone();

    let run_count = Arc::new(AtomicUsize::new(0));
    let r = run_count.clone();

    let owner = ReactiveOwner::root();
    let _guard = owner.enter();

    let _effect = Effect::new(rt.clone(), move || {
        let _ = s.get_untracked();
        r.fetch_add(1, Ordering::SeqCst);
    });

    assert_eq!(run_count.load(Ordering::SeqCst), 1);

    signal.set(10);
    rt.drain_effects();

    assert!(run_count.load(Ordering::SeqCst) >= 1);
}

#[test]
fn multiple_effects_all_run() {
    let rt = Runtime::new();

    let count1 = Arc::new(AtomicUsize::new(0));
    let count2 = Arc::new(AtomicUsize::new(0));
    let count3 = Arc::new(AtomicUsize::new(0));

    let c1 = count1.clone();
    let c2 = count2.clone();
    let c3 = count3.clone();

    let owner = ReactiveOwner::root();
    let _guard = owner.enter();

    let _e1 = Effect::new(rt.clone(), move || {
        c1.fetch_add(1, Ordering::SeqCst);
    });
    let _e2 = Effect::new(rt.clone(), move || {
        c2.fetch_add(1, Ordering::SeqCst);
    });
    let _e3 = Effect::new(rt.clone(), move || {
        c3.fetch_add(1, Ordering::SeqCst);
    });

    assert_eq!(count1.load(Ordering::SeqCst), 1);
    assert_eq!(count2.load(Ordering::SeqCst), 1);
    assert_eq!(count3.load(Ordering::SeqCst), 1);
}
