//! L2 integration test: device-lost deferred during modal size/move loop.
//! With `in_size_move=true`, a forced loss must park the state machine in
//! `DeferredUntilStable` and keep it there across multiple ticks. No retry
//! attempts, no `Renderer::new` calls.
//!
//! The defer-exit scenario lives in its own file
//! (`recovery_size_move_defer_exit_l2.rs`) — running multiple full-recovery
//! scenarios in one test binary leaves wgpu/DXGI state that blocks the next
//! platform/window from completing recovery.

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
const HARD_TIMEOUT: Duration = Duration::from_secs(20);

struct NoopView;
impl View for NoopView {
    fn render(&mut self) -> AnyElement {
        Div::new().into_any()
    }
}

#[test]
fn l2_defer_entry_parks_in_deferred_until_stable() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-l2-defer-entry".into(),
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

    let start = Instant::now();
    let initialized = Cell::new(false);
    let triggered = Cell::new(false);
    let ticks_in_deferred = Cell::new(0u32);
    let initial_gen = Cell::new(0u64);
    let mut view_factory = |_cx: &AppContext| NoopView;

    platform.run(|event| {
        if start.elapsed() > HARD_TIMEOUT {
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
                initial_gen.set(state.renderer_generation().unwrap_or(0));
                initialized.set(true);
                // Enter the modal size/move loop BEFORE triggering loss.
                window.set_in_size_move_for_test(true);
                true
            }
            Event::Wake | Event::WindowRedrawRequested { .. } => true,
            _ => false,
        };
        if !should_tick {
            return;
        }

        state.dispatch_redraw();

        if initialized.get() && !triggered.get() {
            // Force a LuidMigration-origin loss while in size/move.
            state.force_renderer_device_lost_luid_migration_for_test();
            triggered.set(true);
        }

        if triggered.get() {
            match state.current_recovery_state() {
                RecoveryState::DeferredUntilStable { .. } => {
                    ticks_in_deferred.set(ticks_in_deferred.get() + 1);
                    // 8 ticks of dispatch_redraw without leaving
                    // DeferredUntilStable is enough to prove deferral is sticky.
                    if ticks_in_deferred.get() >= 8 {
                        platform.quit();
                        return;
                    }
                }
                RecoveryState::NotLost => {
                    // Allowed BEFORE the loss is observed by dispatch_redraw.
                }
                other => {
                    panic!(
                        "L2 defer: state machine left deferral while in size/move: {:?}",
                        other
                    );
                }
            }
        }

        thread::sleep(TICK_INTERVAL);
        wake_run_loop();
    });

    assert!(triggered.get(), "loss never triggered");
    assert!(
        matches!(
            state.current_recovery_state(),
            RecoveryState::DeferredUntilStable { .. }
        ),
        "expected DeferredUntilStable, got {:?}",
        state.current_recovery_state()
    );
    // The renderer must NOT have rebuilt while we were deferred.
    let final_gen = state.renderer_generation().unwrap_or(0);
    assert_eq!(
        final_gen,
        initial_gen.get(),
        "renderer generation must NOT advance while deferred ({} -> {})",
        initial_gen.get(),
        final_gen
    );
}
