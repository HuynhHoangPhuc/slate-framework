use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use slate_reactive::{Effect, ReactiveOwner, Runtime};

#[test]
fn effect_runs_on_creation_with_owner() {
    let rt = Runtime::new();
    let run_count = Arc::new(AtomicUsize::new(0));
    let r = run_count.clone();

    let owner = ReactiveOwner::root();
    {
        let _guard = owner.enter();
        let _effect = Effect::new(rt.clone(), move || {
            r.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(run_count.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn effect_without_owner_still_runs() {
    let rt = Runtime::new();
    let run_count = Arc::new(AtomicUsize::new(0));
    let r = run_count.clone();

    let _effect = Effect::new(rt.clone(), move || {
        r.fetch_add(1, Ordering::SeqCst);
    });

    assert_eq!(run_count.load(Ordering::SeqCst), 1);
}

#[test]
fn nested_owners_with_effects() {
    let rt = Runtime::new();

    let outer_count = Arc::new(AtomicUsize::new(0));
    let inner_count = Arc::new(AtomicUsize::new(0));

    let oc = outer_count.clone();
    let ic = inner_count.clone();

    let outer_owner = ReactiveOwner::root();
    let _outer_guard = outer_owner.enter();

    let _outer_effect = Effect::new(rt.clone(), move || {
        oc.fetch_add(1, Ordering::SeqCst);
    });

    {
        let inner_owner = ReactiveOwner::root();
        let _inner_guard = inner_owner.enter();

        let _inner_effect = Effect::new(rt.clone(), move || {
            ic.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(outer_count.load(Ordering::SeqCst), 1);
        assert_eq!(inner_count.load(Ordering::SeqCst), 1);
    }

    assert_eq!(outer_count.load(Ordering::SeqCst), 1);
    assert_eq!(inner_count.load(Ordering::SeqCst), 1);
}

#[test]
fn owner_scope_isolation() {
    let _rt = Runtime::new();

    assert!(ReactiveOwner::current().is_none());

    let owner = ReactiveOwner::root();
    {
        let _guard = owner.enter();
        assert!(ReactiveOwner::current().is_some());
    }

    assert!(ReactiveOwner::current().is_none());
}
