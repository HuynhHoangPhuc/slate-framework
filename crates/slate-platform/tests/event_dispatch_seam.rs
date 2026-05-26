//! Verifies the platform event-dispatch funnel carries window-lifecycle events
//! to the installed handler with their `WindowId` intact.
//!
//! The test drives the same `dispatch_event` path every native callback uses,
//! via the `*_for_test` seam, so no real window is created. It asserts the
//! close→destroy ordering and id propagation that the run loop relies on.
//!
//! Platform divergence (not forced into false parity here): on macOS the window
//! is unregistered from the thread-local registry *before* `WindowDestroyed`
//! fires; on Windows `WM_DESTROY` couples the destroy event to
//! `PostQuitMessage`. Both still surface `WindowCloseRequested` then
//! `WindowDestroyed` through this same funnel, which is what the seam pins.

#![cfg(all(any(target_os = "macos", target_os = "windows"), feature = "test-hooks"))]

use std::cell::RefCell;
use std::rc::Rc;

use slate_platform::{
    clear_event_handler_for_test, dispatch_event_for_test, install_event_handler_for_test, Event,
    WindowId,
};

#[test]
fn lifecycle_events_reach_handler_with_window_id() {
    let recorded: Rc<RefCell<Vec<(&'static str, WindowId)>>> = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&recorded);

    install_event_handler_for_test(Box::new(move |event| match event {
        Event::WindowCloseRequested { window } => sink.borrow_mut().push(("close", window)),
        Event::WindowDestroyed { window } => sink.borrow_mut().push(("destroy", window)),
        _ => {}
    }));

    let w = WindowId(7);
    dispatch_event_for_test(Event::WindowCloseRequested { window: w });
    dispatch_event_for_test(Event::WindowDestroyed { window: w });

    clear_event_handler_for_test();

    let seen = recorded.borrow();
    assert_eq!(
        seen.as_slice(),
        &[("close", WindowId(7)), ("destroy", WindowId(7))],
        "close→destroy must reach the handler in order with the window id preserved"
    );
}

#[test]
fn reentrant_dispatch_does_not_panic() {
    // Win32 reproduction: a handler that triggers `CreateWindowExW` re-enters
    // `dispatch_event` through wndproc (WM_NCCREATE → WM_SIZE) while the outer
    // handler still holds the HANDLER borrow. A naive `borrow_mut` panics here
    // and the wndproc trampoline turns the panic into `process::abort` — the
    // exact crash from the Ctrl+N path in the two-window example. The funnel
    // must drop the re-entrant event instead of panicking.
    let outer_count: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let counter = Rc::clone(&outer_count);

    install_event_handler_for_test(Box::new(move |event| {
        *counter.borrow_mut() += 1;
        if let Event::WindowCloseRequested { .. } = event {
            // Simulate the CreateWindowExW → synchronous wndproc re-entry.
            dispatch_event_for_test(Event::WindowDestroyed { window: WindowId(99) });
        }
    }));

    dispatch_event_for_test(Event::WindowCloseRequested { window: WindowId(1) });
    clear_event_handler_for_test();

    // Outer event was delivered exactly once; the re-entrant inner event is
    // silently dropped (the contract documented on `dispatch_event`).
    assert_eq!(*outer_count.borrow(), 1, "re-entrant event must be dropped, not double-delivered or panic");
}

#[test]
fn dispatch_is_noop_after_handler_cleared() {
    let recorded: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
    let sink = Rc::clone(&recorded);

    install_event_handler_for_test(Box::new(move |_| *sink.borrow_mut() += 1));
    dispatch_event_for_test(Event::WindowDestroyed { window: WindowId(1) });
    clear_event_handler_for_test();
    dispatch_event_for_test(Event::WindowDestroyed { window: WindowId(1) });

    assert_eq!(*recorded.borrow(), 1, "events after clear must not be dispatched");
}
