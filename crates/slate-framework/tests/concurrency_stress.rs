//! Concurrency stress tests for the reactive system.
//!
//! Run in release mode 10× for flake detection:
//! ```sh
//! for i in {1..10}; do cargo test -p slate-framework concurrency_stress --release; done
//! ```

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use slate_reactive::{Runtime, Signal, with_observer};

/// 10 threads × 1000 `set()` calls each with concurrent UI-thread render simulation.
///
/// Asserts:
/// - No deadlock (test completes within 5s)
/// - No lost notifications (dirty bit set after all sets)
/// - No panic
#[test]
fn ten_threads_thousand_sets_with_render_loop() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0u64);
    let redraw_count = Arc::new(AtomicUsize::new(0));

    // Install redraw callback
    let count = redraw_count.clone();
    rt.install_redraw(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));

    let mut handles = Vec::new();

    // Spawn 10 writer threads
    for thread_id in 0..10u64 {
        let s = signal.clone();
        handles.push(thread::spawn(move || {
            for i in 0..1000u64 {
                s.set(thread_id * 1000 + i);
            }
        }));
    }

    // Spawn UI-thread render loop simulation
    let render_handle = {
        let rt = rt.clone();
        let s = signal.clone();
        thread::spawn(move || {
            let observer = rt.next_observer_id();
            for _ in 0..100 {
                // Simulate render: drain dirty, read signal under observer scope
                let _ = rt.drain_dirty();
                with_observer(observer, || {
                    let _ = s.get();
                });
                rt.drain_effects();
                thread::sleep(Duration::from_millis(1));
            }
        })
    };

    // Wait for all writers (with timeout guard)
    let start = Instant::now();
    for h in handles {
        h.join().expect("writer thread should not panic");
        if start.elapsed() > Duration::from_secs(5) {
            panic!("deadlock detected: test exceeded 5s timeout");
        }
    }

    // Wait for render thread
    render_handle
        .join()
        .expect("render thread should not panic");

    // Verify dirty bit was set at least once
    assert!(
        rt.drain_dirty() || redraw_count.load(Ordering::SeqCst) > 0,
        "at least one set should have triggered dirty or redraw callback"
    );
}

/// Single-thread × 100k `set()` calls in rapid succession.
///
/// Exercises brainstorm §6 row 7 risk: 10kHz background set flood.
/// Should complete without deadlock, no lost notifications.
#[test]
fn single_thread_100k_flood() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0u64);
    let redraw_count = Arc::new(AtomicUsize::new(0));

    let count = redraw_count.clone();
    rt.install_redraw(Arc::new(move || {
        count.fetch_add(1, Ordering::SeqCst);
    }));

    let start = Instant::now();

    // 100,000 sets in rapid succession
    for i in 0..100_000u64 {
        signal.set(i);
    }

    let elapsed = start.elapsed();

    // Should complete quickly (< 1s on most machines)
    assert!(
        elapsed < Duration::from_secs(5),
        "100k sets took too long: {:?}",
        elapsed
    );

    // Dirty bit coalescing: should have triggered at least one redraw
    let redraws = redraw_count.load(Ordering::SeqCst);
    assert!(redraws > 0, "100k sets should trigger at least one redraw");

    // But NOT 100k redraws (coalescing working)
    assert!(
        redraws < 10_000,
        "redraw should coalesce: got {} redraws for 100k sets",
        redraws
    );

    // Verify final value
    assert_eq!(signal.get_untracked(), 99_999);
}

/// Concurrent get + set with update() (atomic read-modify-write pattern).
///
/// Tests that update() is thread-safe and no values are lost.
#[test]
fn concurrent_update_accumulator() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0u64);

    let mut handles = Vec::new();

    // 10 threads, each doing 1000 increments via update()
    for _ in 0..10 {
        let s = signal.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..1000 {
                s.update(|n| *n += 1);
            }
        }));
    }

    for h in handles {
        h.join().expect("thread should not panic");
    }

    // All increments should be counted (no lost updates)
    assert_eq!(signal.get_untracked(), 10_000);
}

/// Mixed observer registration + signal mutation from multiple threads.
///
/// Tests that observer registry doesn't deadlock under concurrent mutation.
#[test]
fn concurrent_observer_registration() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0u64);

    let mut handles = Vec::new();

    // 5 threads registering observers and reading
    for _ in 0..5 {
        let rt = rt.clone();
        let s = signal.clone();
        handles.push(thread::spawn(move || {
            for _ in 0..100 {
                let obs = rt.next_observer_id();
                with_observer(obs, || {
                    let _ = s.get(); // Subscribe
                });
            }
        }));
    }

    // 5 threads setting values
    for thread_id in 0..5u64 {
        let s = signal.clone();
        handles.push(thread::spawn(move || {
            for i in 0..100u64 {
                s.set(thread_id * 100 + i);
            }
        }));
    }

    for h in handles {
        h.join().expect("thread should not panic");
    }
}

/// High-contention scenario: many threads hammering a single signal.
///
/// Tests lock fairness and starvation resistance.
#[test]
fn high_contention_single_signal() {
    let rt = Runtime::new();
    let signal = Signal::new(rt.clone(), 0u64);
    let completed = Arc::new(AtomicU64::new(0));

    let mut handles = Vec::new();

    // 20 threads, each doing 500 operations (mix of get/set)
    for thread_id in 0..20u64 {
        let s = signal.clone();
        let done = completed.clone();
        handles.push(thread::spawn(move || {
            for i in 0..500u64 {
                if i % 2 == 0 {
                    s.set(thread_id * 500 + i);
                } else {
                    let _ = s.get_untracked();
                }
            }
            done.fetch_add(1, Ordering::SeqCst);
        }));
    }

    // Timeout guard: if any thread starves, this catches it
    let start = Instant::now();
    for h in handles {
        let result = h.join();
        if start.elapsed() > Duration::from_secs(10) {
            panic!("timeout: possible starvation or deadlock");
        }
        result.expect("thread should not panic");
    }

    // All threads completed
    assert_eq!(completed.load(Ordering::SeqCst), 20);
}
