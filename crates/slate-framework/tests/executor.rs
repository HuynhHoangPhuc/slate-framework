//! Integration tests for async executor infrastructure.
//! Includes trybuild compile-fail driver for !Send assertions.

use slate_framework::executor::{
    BackgroundExecutor, Executor, ForegroundExecutor, RedrawRequester,
};

#[test]
fn redraw_requester_noop() {
    let requester = RedrawRequester::noop();
    requester.request();
}

#[test]
fn redraw_requester_with_callback() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let called = Arc::new(AtomicBool::new(false));
    let called_clone = called.clone();

    let requester = RedrawRequester::new(move || {
        called_clone.store(true, Ordering::SeqCst);
    });

    requester.request();
    assert!(called.load(Ordering::SeqCst));
}

#[test]
fn foreground_executor_new() {
    let requester = RedrawRequester::noop();
    let executor = ForegroundExecutor::new(requester);
    // Poll should not block on empty executor
    executor.poll();
}

#[test]
fn foreground_executor_try_tick_empty() {
    let requester = RedrawRequester::noop();
    let executor = ForegroundExecutor::new(requester);
    // No tasks, should return false
    assert!(!executor.try_tick());
}

#[test]
fn background_executor_new() {
    let requester = RedrawRequester::noop();
    let _executor = BackgroundExecutor::new(requester);
}

#[test]
fn executor_aggregate_new() {
    let requester = RedrawRequester::noop();
    let executor = Executor::new(requester);
    executor.poll_foreground();
}

#[test]
fn background_executor_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<BackgroundExecutor>();
}

#[test]
fn redraw_requester_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<RedrawRequester>();
}

// Trybuild compile-fail tests
// Skip on Windows: error message formatting differs between platforms
#[test]
#[cfg_attr(target_os = "windows", ignore)]
fn compile_fail_tests() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile_fail/*.rs");
}
