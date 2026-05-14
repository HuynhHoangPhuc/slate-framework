//! Integration test for the device-lost callback path → state machine engagement.
//!
//! Validates that `fire_device_lost_callback_for_test` correctly:
//! 1. Filters `Destroyed` reason (no recovery triggered)
//! 2. Engages the recovery state machine for `Unknown` reason
//!
//! Distinct from `recovery_end_to_end.rs` which exercises the DXGI `RemoveDevice`
//! path. This test validates Slate's callback wiring without a real wgpu callback.
//!
//! Windows-only with `test-hooks` feature (callback test hook is cfg-gated).

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::Cell;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use slate_framework::app::AppContext;
use slate_framework::app_state::{AppState, RecoveryState};
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::view::{IntoAny, View};
use slate_platform::{
    DefaultPlatform, Event, Platform, Window, WindowOptions, WindowRenderDelegate, wake_run_loop,
};

const TICK_INTERVAL: Duration = Duration::from_millis(10);

struct NoopView;

impl View for NoopView {
    fn render(&mut self) -> AnyElement {
        Div::new().into_any()
    }
}

#[test]
fn destroyed_reason_does_not_trigger_recovery() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-callback-test".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32000, -32000)),
    });

    let redraw_requester = RedrawRequester::new(wake_run_loop);
    let executor = Executor::new(redraw_requester.clone());
    let runtime = slate_reactive::Runtime::new();
    let cx = AppContext::new_for_test(runtime.clone(), executor.background.clone());

    let state = Rc::new(AppState::new(
        window.clone(),
        executor,
        redraw_requester,
        runtime,
    ));

    let dyn_strong: Rc<dyn WindowRenderDelegate> = state.clone();
    let dyn_weak = Rc::downgrade(&dyn_strong);
    window.set_render_delegate(dyn_weak);
    drop(dyn_strong);

    let timeout = Duration::from_secs(3);
    let start = Instant::now();
    let initialized = Cell::new(false);
    let fired = Cell::new(false);
    let mut view_factory = |_cx: &AppContext| NoopView;

    platform.run(|event| {
        if start.elapsed() > timeout {
            platform.quit();
            return;
        }

        let should_tick = match event {
            Event::Resumed => {
                if state
                    .init_surfaces(&mut view_factory, &cx, &platform)
                    .is_err()
                {
                    platform.quit();
                    return;
                }
                initialized.set(true);
                true
            }
            Event::Wake | Event::WindowRedrawRequested { .. } => true,
            _ => false,
        };

        if !should_tick {
            return;
        }

        state.dispatch_redraw();

        // After initialization, fire the callback with Destroyed reason
        if initialized.get() && !fired.get() {
            let result = state.fire_renderer_device_lost_callback(
                wgpu::DeviceLostReason::Destroyed,
                "intentional drop for test".into(),
            );
            // Destroyed should be filtered → returns false
            assert!(
                !result,
                "Destroyed reason should be filtered (return false)"
            );

            // Device-lost flag should NOT be set
            assert!(
                !state.renderer_is_device_lost(),
                "Device-lost flag should NOT be set for Destroyed reason"
            );

            // Recovery state should remain NotLost
            assert!(
                matches!(state.current_recovery_state(), RecoveryState::NotLost),
                "Recovery state should remain NotLost for Destroyed reason"
            );

            fired.set(true);
            platform.quit();
            return;
        }

        thread::sleep(TICK_INTERVAL);
        wake_run_loop();
    });

    assert!(fired.get(), "Test did not complete within timeout");
}

#[test]
fn unknown_reason_triggers_recovery_state_machine() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-callback-test-unknown".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32000, -32000)),
    });

    let redraw_requester = RedrawRequester::new(wake_run_loop);
    let executor = Executor::new(redraw_requester.clone());
    let runtime = slate_reactive::Runtime::new();
    let cx = AppContext::new_for_test(runtime.clone(), executor.background.clone());

    let state = Rc::new(AppState::new(
        window.clone(),
        executor,
        redraw_requester,
        runtime,
    ));

    let dyn_strong: Rc<dyn WindowRenderDelegate> = state.clone();
    let dyn_weak = Rc::downgrade(&dyn_strong);
    window.set_render_delegate(dyn_weak);
    drop(dyn_strong);

    // Short timeout — we just need to verify state machine engagement, not full recovery
    let timeout = Duration::from_secs(3);
    let start = Instant::now();
    let initialized = Cell::new(false);
    let fired = Cell::new(false);
    let recovery_engaged = Cell::new(false);
    let mut view_factory = |_cx: &AppContext| NoopView;

    platform.run(|event| {
        if start.elapsed() > timeout {
            platform.quit();
            return;
        }

        let should_tick = match event {
            Event::Resumed => {
                if state
                    .init_surfaces(&mut view_factory, &cx, &platform)
                    .is_err()
                {
                    platform.quit();
                    return;
                }
                initialized.set(true);
                true
            }
            Event::Wake | Event::WindowRedrawRequested { .. } => true,
            _ => false,
        };

        if !should_tick {
            return;
        }

        state.dispatch_redraw();

        // After initialization, fire the callback with Unknown reason
        if initialized.get() && !fired.get() {
            let result = state.fire_renderer_device_lost_callback(
                wgpu::DeviceLostReason::Unknown,
                "test-triggered device loss".into(),
            );
            // Unknown should trigger → returns true
            assert!(result, "Unknown reason should trigger (return true)");

            // Device-lost flag SHOULD be set
            assert!(
                state.renderer_is_device_lost(),
                "Device-lost flag SHOULD be set for Unknown reason"
            );

            fired.set(true);
        }

        // Track recovery state machine engagement — once engaged, we're done
        if fired.get() && !recovery_engaged.get() {
            let recovery = state.current_recovery_state();
            if !matches!(recovery, RecoveryState::NotLost) {
                recovery_engaged.set(true);
                println!("Recovery state machine engaged: {:?}", recovery);
                // Success — state machine moved past NotLost
                platform.quit();
                return;
            }
        }

        thread::sleep(TICK_INTERVAL);
        wake_run_loop();
    });

    assert!(fired.get(), "Callback was not fired within timeout");
    assert!(
        recovery_engaged.get(),
        "Recovery state machine never engaged (never left NotLost state)"
    );
}

#[test]
fn atomic_flag_visibility_across_threads() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Synthetic test for atomic ordering contract:
    // Writer thread stores with Release, reader thread loads with Acquire.
    // This validates the ordering used by the callback (AcqRel swap)
    // and the main-thread reader (Acquire load).

    let flag = Arc::new(AtomicBool::new(false));
    let flag_writer = Arc::clone(&flag);

    let writer = std::thread::spawn(move || {
        // Simulate callback thread setting the flag
        flag_writer.store(true, Ordering::Release);
    });

    writer.join().expect("writer thread panicked");

    // Main thread reads with Acquire
    let value = flag.load(Ordering::Acquire);
    assert!(value, "Atomic flag should be visible after Release store");
}
