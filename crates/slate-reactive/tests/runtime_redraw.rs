use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use slate_reactive::Runtime;

#[test]
fn install_and_trigger_redraw() {
    let rt = Runtime::new();
    let call_count = Arc::new(AtomicUsize::new(0));

    let count = call_count.clone();
    rt.install_redraw(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));

    assert_eq!(call_count.load(Ordering::SeqCst), 0);

    rt.mark_dirty();
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[test]
fn redraw_coalesced_without_drain() {
    let rt = Runtime::new();
    let call_count = Arc::new(AtomicUsize::new(0));

    let count = call_count.clone();
    rt.install_redraw(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));

    rt.mark_dirty();
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    rt.mark_dirty();
    rt.mark_dirty();
    rt.mark_dirty();
    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[test]
fn redraw_fires_again_after_drain() {
    let rt = Runtime::new();
    let call_count = Arc::new(AtomicUsize::new(0));

    let count = call_count.clone();
    rt.install_redraw(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));

    rt.mark_dirty();
    assert_eq!(call_count.load(Ordering::SeqCst), 1);

    assert!(rt.drain_dirty());

    rt.mark_dirty();
    assert_eq!(call_count.load(Ordering::SeqCst), 2);
}

#[test]
fn no_callback_installed() {
    let rt = Runtime::new();

    rt.mark_dirty();
    assert!(rt.drain_dirty());
}
