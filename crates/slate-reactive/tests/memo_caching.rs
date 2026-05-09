use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use slate_reactive::{Memo, Runtime};

#[test]
fn memo_caches_on_get() {
    let rt = Runtime::new();
    let compute_count = Arc::new(AtomicUsize::new(0));
    let count = compute_count.clone();

    let memo = Memo::new(rt, move || {
        count.fetch_add(1, Ordering::SeqCst);
        42
    });

    assert_eq!(memo.get_untracked(), 42);
    assert_eq!(compute_count.load(Ordering::SeqCst), 1);

    assert_eq!(memo.get_untracked(), 42);
    assert_eq!(compute_count.load(Ordering::SeqCst), 1);

    assert_eq!(memo.get_untracked(), 42);
    assert_eq!(compute_count.load(Ordering::SeqCst), 1);
}

#[test]
fn memo_clone_shares_cache() {
    let rt = Runtime::new();
    let compute_count = Arc::new(AtomicUsize::new(0));
    let count = compute_count.clone();

    let memo = Memo::new(rt, move || {
        count.fetch_add(1, Ordering::SeqCst);
        "computed"
    });

    let memo2 = memo.clone();

    assert_eq!(memo.get_untracked(), "computed");
    assert_eq!(compute_count.load(Ordering::SeqCst), 1);

    assert_eq!(memo2.get_untracked(), "computed");
    assert_eq!(
        compute_count.load(Ordering::SeqCst),
        1,
        "clone should share cache"
    );
}

#[test]
fn memo_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Memo<u32>>();
    assert_send_sync::<Memo<String>>();
}

#[test]
fn memo_computes_lazily() {
    let rt = Runtime::new();
    let compute_count = Arc::new(AtomicUsize::new(0));
    let count = compute_count.clone();

    let _memo = Memo::new(rt, move || {
        count.fetch_add(1, Ordering::SeqCst);
        99
    });

    assert_eq!(
        compute_count.load(Ordering::SeqCst),
        0,
        "memo should not compute on creation"
    );
}

#[test]
fn memo_with_complex_value() {
    let rt = Runtime::new();

    let memo = Memo::new(rt, || vec![1, 2, 3, 4, 5]);

    assert_eq!(memo.get_untracked(), vec![1, 2, 3, 4, 5]);
    assert_eq!(memo.get_untracked(), vec![1, 2, 3, 4, 5]);
}
