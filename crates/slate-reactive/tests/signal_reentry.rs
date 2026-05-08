use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use slate_reactive::{Runtime, Signal};

#[test]
fn set_during_notify_does_not_deadlock() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0u32);

    let call_count = Arc::new(AtomicUsize::new(0));
    let count = call_count.clone();
    let s = signal.clone();

    rt.install_redraw(Arc::new(move || {
        let c = count.fetch_add(1, Ordering::SeqCst);
        if c < 5 {
            s.set(c as u32 + 1);
        }
    }));

    signal.set(1);

    let final_count = call_count.load(Ordering::SeqCst);
    assert!(
        final_count >= 1,
        "redraw should have been called at least once"
    );
}

#[test]
fn nested_signal_set_in_redraw() {
    let rt = Runtime::new();
    let signal_a = Signal::new(rt.clone(), 0u32);
    let signal_b = Signal::new(rt.clone(), 0u32);

    let call_count = Arc::new(AtomicUsize::new(0));
    let count = call_count.clone();
    let sb = signal_b.clone();

    rt.install_redraw(Arc::new(move || {
        let c = count.fetch_add(1, Ordering::SeqCst);
        if c == 0 {
            sb.set(100);
        }
    }));

    signal_a.set(1);

    assert_eq!(signal_b.get_untracked(), 100);
    assert!(call_count.load(Ordering::SeqCst) >= 1);
}

#[test]
fn update_during_redraw() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), vec![1u32]);

    let call_count = Arc::new(AtomicUsize::new(0));
    let count = call_count.clone();
    let s = signal.clone();

    rt.install_redraw(Arc::new(move || {
        let c = count.fetch_add(1, Ordering::SeqCst);
        if c < 3 {
            s.update(|v| v.push(c as u32 + 2));
        }
    }));

    signal.update(|v| v[0] = 10);

    let final_value = signal.get_untracked();
    assert!(final_value.len() >= 1);
    assert_eq!(final_value[0], 10);
}
