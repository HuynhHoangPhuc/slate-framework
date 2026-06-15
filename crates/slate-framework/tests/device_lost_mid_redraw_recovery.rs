//! Integration test: a device loss discovered *mid-redraw* aborts the frame
//! and the recovery state machine converges back to a rendering window.
//!
//! Regression lock for the Windows device-lost crash where `run_redraw` kept
//! issuing GPU work (layout → paint uploads → present) after the device was
//! already removed, producing `Placed buffer creation failed 0x887A0005` and a
//! process abort. After the fix:
//!   - `run_redraw` returns `RedrawOutcome::DeviceLost` and stops immediately
//!     (proved via `run_redraw_is_device_lost_for_test`), and
//!   - driving `dispatch_redraw` converges `NotLost → … → Recovered → NotLost`
//!     with an incremented renderer generation (full recovery, per the plan's
//!     raised acceptance bar — not merely "recovery entered").
//!
//! Uses the synthetic `fire_renderer_device_lost_callback` (sets the device-lost
//! flag + telemetry without a real `RemoveDevice()`), so the GPU stays usable
//! and recovery's `Renderer::new` rebuild deterministically succeeds. Windows +
//! test-hooks gated; empty elsewhere.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::Cell;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use slate_framework::app::AppContext;
use slate_framework::app_state::window_state::WindowState;
use slate_framework::app_state::{AppSignal, AppState, RecoveryState};
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::erased_view::ErasedView;
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
fn mid_redraw_device_loss_aborts_then_recovers() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-mid-redraw-recovery".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32000, -32000)),
        ..Default::default()
    });
    let redraw_requester = RedrawRequester::new(wake_run_loop);
    let executor = Executor::new(redraw_requester.clone());
    let runtime = slate_reactive::Runtime::new();
    let cx = AppContext::new_for_test(runtime.clone(), executor.background.clone());
    let window_id = window.id();

    let state = Rc::new(AppState::new(
        executor,
        redraw_requester.clone(),
        runtime.clone(),
    ));
    state
        .windows
        .borrow_mut()
        .insert(window_id, WindowState::new(window.clone(), runtime));
    state.register_redraw_requester_for_test(window_id, redraw_requester);

    let dyn_strong: Rc<dyn WindowRenderDelegate> = state.clone();
    window.set_render_delegate(Rc::downgrade(&dyn_strong));
    drop(dyn_strong);

    let start = Instant::now();
    // Observations recorded inside the loop, asserted after it returns.
    let initial_gen = Cell::new(0u64);
    let baseline_not_lost = Cell::new(false);
    let abort_observed = Cell::new(false);
    let recovered = Cell::new(false);
    let gave_up = Cell::new(false);
    let request_quit = Cell::new(false);
    let triggered = Cell::new(false);
    let mut view_factory = |_cx: &AppContext| Box::new(NoopView) as Box<dyn ErasedView>;

    platform.run(|event| {
        if start.elapsed() > HARD_TIMEOUT {
            platform.quit();
            return;
        }

        match event {
            Event::Resumed => {
                if state
                    .init_surfaces(window_id, &mut view_factory, &cx, &platform)
                    .is_err()
                {
                    platform.quit();
                    return;
                }
                // Baseline: one clean frame, healthy device.
                state.dispatch_redraw(window_id);
                initial_gen.set(state.renderer_generation_for(window_id).unwrap_or(0));
                baseline_not_lost.set(
                    matches!(state.current_recovery_state(), RecoveryState::NotLost)
                        && !state.renderer_is_device_lost_for(window_id),
                );

                // Synthetic mid-frame loss: set the device-lost flag the way the
                // real wgpu callback would, WITHOUT removing the GPU.
                assert!(
                    state.fire_renderer_device_lost_callback(
                        wgpu::DeviceLostReason::Unknown,
                        "synthetic mid-frame loss".to_owned(),
                    ),
                    "fire_renderer_device_lost_callback returned false — renderer not initialized"
                );

                // The inner pipeline must ABORT (not plow through paint/present)
                // now that the device is lost.
                abort_observed.set(state.run_redraw_is_device_lost_for_test(window_id));
                triggered.set(true);

                schedule_tick();
            }
            Event::Wake | Event::WindowRedrawRequested { .. } => {
                if !triggered.get() {
                    schedule_tick();
                    return;
                }
                let sig = state.dispatch_redraw(window_id);
                if matches!(sig, AppSignal::RequestQuit) {
                    request_quit.set(true);
                    platform.quit();
                    return;
                }
                let recovery = state.current_recovery_state();
                let gen_now = state.renderer_generation_for(window_id).unwrap_or(0);
                if matches!(recovery, RecoveryState::GiveUp { .. }) {
                    gave_up.set(true);
                    platform.quit();
                    return;
                }
                if matches!(recovery, RecoveryState::NotLost)
                    && gen_now > initial_gen.get()
                    && !state.renderer_is_device_lost_for(window_id)
                {
                    recovered.set(true);
                    platform.quit();
                    return;
                }
                schedule_tick();
            }
            _ => {}
        }
    });

    assert!(
        baseline_not_lost.get(),
        "baseline frame should leave the device healthy and NotLost"
    );
    assert!(
        abort_observed.get(),
        "run_redraw must return RedrawOutcome::DeviceLost (abort) once the device is lost mid-frame"
    );
    assert!(!request_quit.get(), "single transient loss must not RequestQuit");
    assert!(!gave_up.get(), "single transient loss must not reach GiveUp");
    assert!(
        recovered.get(),
        "recovery must converge back to NotLost with a higher renderer generation \
         (full recovery, not merely 'recovery entered')"
    );
    assert!(
        state.renderer_generation_for(window_id).unwrap_or(0) > initial_gen.get(),
        "renderer generation must increment across recovery"
    );
}

fn schedule_tick() {
    thread::sleep(TICK_INTERVAL);
    wake_run_loop();
}
