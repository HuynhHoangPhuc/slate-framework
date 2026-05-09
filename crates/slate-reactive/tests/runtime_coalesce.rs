use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use slate_reactive::Runtime;

#[test]
fn thousand_threads_coalesce() {
    let rt = Runtime::new();
    let call_count = Arc::new(AtomicUsize::new(0));

    let count = call_count.clone();
    rt.install_redraw(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));

    let mut handles = Vec::new();

    for _ in 0..1000 {
        let rt_clone = rt.clone();
        handles.push(thread::spawn(move || {
            rt_clone.mark_dirty();
        }));
    }

    for h in handles {
        h.join().expect("thread should not panic");
    }

    let final_count = call_count.load(Ordering::SeqCst);

    assert!(final_count >= 1, "callback should fire at least once");
    assert!(
        final_count < 100,
        "callback should be coalesced (got {final_count}, expected < 100)"
    );

    assert!(rt.drain_dirty(), "dirty bit should be set");
}

#[test]
fn concurrent_mark_and_drain() {
    let rt = Runtime::new();
    let call_count = Arc::new(AtomicUsize::new(0));

    let count = call_count.clone();
    rt.install_redraw(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));

    let mut handles = Vec::new();

    for i in 0..100 {
        let rt_clone = rt.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..10 {
                rt_clone.mark_dirty();
                if i % 10 == 0 {
                    rt_clone.drain_dirty();
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("thread should not panic");
    }

    let final_count = call_count.load(Ordering::SeqCst);
    assert!(final_count >= 1, "callback should fire at least once");
}
