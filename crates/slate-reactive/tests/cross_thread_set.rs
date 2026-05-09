use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use slate_reactive::{Runtime, Signal};

#[test]
fn ten_threads_thousand_sets() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0u64);

    let mut handles = Vec::new();

    for thread_id in 0..10u64 {
        let s = signal.clone();
        handles.push(thread::spawn(move || {
            for i in 0..1000u64 {
                s.set(thread_id * 1000 + i);
            }
        }));
    }

    for h in handles {
        h.join().expect("thread should not panic");
    }

    assert!(rt.drain_dirty(), "dirty bit should be set after all sets");

    let final_value = signal.get_untracked();
    assert!(final_value < 10_000, "final value should be valid");
}

#[test]
fn concurrent_get_and_set() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0u64);
    let reads = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();

    for _ in 0..5 {
        let s = signal.clone();
        handles.push(thread::spawn(move || {
            for i in 0..1000 {
                s.set(i);
            }
        }));
    }

    for _ in 0..5 {
        let s = signal.clone();
        let r = reads.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..1000 {
                let _ = s.get_untracked();
                r.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for h in handles {
        h.join().expect("thread should not panic");
    }

    assert_eq!(reads.load(Ordering::Relaxed), 5000);
}

#[test]
fn set_from_background_triggers_redraw() {
    use std::sync::atomic::AtomicUsize;

    let rt = Runtime::new();
    let call_count = Arc::new(AtomicUsize::new(0));

    let count = call_count.clone();
    rt.install_redraw(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));

    let signal = Signal::new(rt.clone(), 0);

    let s = signal.clone();
    let handle = thread::spawn(move || {
        s.set(42);
    });

    handle.join().unwrap();

    assert!(call_count.load(Ordering::SeqCst) >= 1);
    assert_eq!(signal.get_untracked(), 42);
}

#[test]
fn no_deadlock_under_contention() {
    use std::time::Duration;

    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0u32);

    let mut handles = Vec::new();

    for _ in 0..20 {
        let s = signal.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..500 {
                s.update(|n| *n = n.wrapping_add(1));
                thread::yield_now();
            }
        }));
    }

    let timeout = thread::spawn(move || {
        thread::sleep(Duration::from_secs(5));
    });

    for h in handles {
        h.join().expect("worker thread should not panic");
    }

    drop(timeout);
}
