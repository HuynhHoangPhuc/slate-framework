//! L2 integration test (own file): leaving the modal size/move loop via
//! `on_size_move_end` transitions `DeferredUntilStable → CooldownGate`,
//! clears `last_adapter_check_at`, and recovery proceeds normally.
//!
//! Lives in its own file: running full recovery scenarios in the same
//! binary as other recovery tests leaves wgpu/DXGI state that blocks the
//! next platform/window from completing recovery.

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
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new().into_any()
    }
}

#[test]
fn l2_defer_exit_resumes_recovery() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-l2-defer-exit".into(),
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
    let exit_called = Cell::new(false);
    let saw_cooldown = Cell::new(false);
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
                window.set_in_size_move_for_test(true);
                true
            }
            Event::Wake | Event::WindowRedrawRequested { .. } => true,
            _ => false,
        };
        if !should_tick {
            return;
        }

        state.dispatch_redraw(state.window_id_for_test());

        if initialized.get() && !triggered.get() {
            state.force_renderer_device_lost_luid_migration_for_test();
            triggered.set(true);
        }

        if triggered.get()
            && !exit_called.get()
            && matches!(
                state.current_recovery_state(),
                RecoveryState::DeferredUntilStable { .. }
            )
        {
            window.set_in_size_move_for_test(false);
            <AppState<NoopView> as WindowRenderDelegate>::on_size_move_end(&state, window.id());
            exit_called.set(true);
        }

        if exit_called.get() {
            if matches!(
                state.current_recovery_state(),
                RecoveryState::CooldownGate { .. }
            ) {
                saw_cooldown.set(true);
            }

            let recovered = matches!(state.current_recovery_state(), RecoveryState::NotLost)
                && state.renderer_generation().unwrap_or(0) > initial_gen.get();
            if recovered {
                platform.quit();
                return;
            }
            if matches!(state.current_recovery_state(), RecoveryState::GiveUp { .. }) {
                platform.quit();
                return;
            }
        }

        thread::sleep(TICK_INTERVAL);
        wake_run_loop();
    });

    assert!(triggered.get(), "loss never triggered");
    assert!(exit_called.get(), "on_size_move_end was never called");
    let final_gen = state.renderer_generation().unwrap_or(0);
    let final_state = state.current_recovery_state();
    assert!(
        matches!(final_state, RecoveryState::NotLost) && final_gen > initial_gen.get(),
        "L2 defer-exit must recover (gen advanced, state NotLost). Got state={:?} gen {} -> {}",
        final_state,
        initial_gen.get(),
        final_gen
    );
    assert!(
        saw_cooldown.get(),
        "expected to observe at least one CooldownGate tick after defer exit"
    );
}
