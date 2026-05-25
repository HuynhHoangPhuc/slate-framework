//! L1 integration test (own file): two `WgpuCallback` losses within 5
//! seconds must escalate to `GiveUp { reason: WgpuCallback }`.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::Cell;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use slate_framework::DeviceLossReason;
use slate_framework::app::AppContext;
use slate_framework::app_state::{AppState, RecoveryState};
use slate_framework::app_state::window_state::WindowState;
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
fn l1_wgpu_callback_flap_gives_up() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-l1-wgpu-flap".into(),
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
    let window_id = window.id();

    let state = Rc::new(AppState::new(executor, redraw_requester.clone(), runtime.clone()));
    {
        state.windows.borrow_mut().insert(window_id, WindowState::new(window.clone(), runtime));
    }
    state.register_redraw_requester_for_test(window_id, redraw_requester);

    let dyn_strong: Rc<dyn WindowRenderDelegate> = state.clone();
    let dyn_weak = Rc::downgrade(&dyn_strong);
    window.set_render_delegate(dyn_weak);
    drop(dyn_strong);

    let start = Instant::now();
    let initialized = Cell::new(false);
    let losses_fired = Cell::new(0u32);
    let initial_gen = Cell::new(0u64);
    let last_known_gen = Cell::new(0u64);
    let mut view_factory = |_cx: &AppContext| Box::new(NoopView) as Box<dyn ErasedView>;

    platform.run(|event| {
        if start.elapsed() > HARD_TIMEOUT {
            platform.quit();
            return;
        }
        let should_tick = match event {
            Event::Resumed => {
                if state
                    .init_surfaces(window_id, &mut view_factory, &cx, &platform)
                    .is_err()
                {
                    platform.quit();
                    return;
                }
                initial_gen.set(state.renderer_generation().unwrap_or(0));
                last_known_gen.set(initial_gen.get());
                initialized.set(true);
                true
            }
            Event::Wake | Event::WindowRedrawRequested { .. } => true,
            _ => false,
        };
        if !should_tick {
            return;
        }

        state.dispatch_redraw(window_id);

        if initialized.get() && losses_fired.get() < 2 {
            let recovered = matches!(state.current_recovery_state(), RecoveryState::NotLost)
                && state.renderer_generation().unwrap_or(0) > last_known_gen.get();
            let first = losses_fired.get() == 0;
            if first || recovered {
                state.fire_renderer_device_lost_callback(
                    wgpu::DeviceLostReason::Unknown,
                    format!("synthetic WgpuCallback #{}", losses_fired.get() + 1),
                );
                losses_fired.set(losses_fired.get() + 1);
                if recovered {
                    last_known_gen.set(state.renderer_generation().unwrap_or(0));
                }
            }
        }

        if matches!(state.current_recovery_state(), RecoveryState::GiveUp { .. }) {
            platform.quit();
            return;
        }

        thread::sleep(TICK_INTERVAL);
        wake_run_loop();
    });

    let final_state = state.current_recovery_state();
    assert_eq!(
        losses_fired.get(),
        2,
        "did not fire both WgpuCallback losses"
    );
    match final_state {
        RecoveryState::GiveUp { reason } => assert_eq!(
            reason,
            DeviceLossReason::WgpuCallback,
            "GiveUp reason must be WgpuCallback"
        ),
        other => panic!("L1: 2× WgpuCallback must GiveUp, got {:?}", other),
    }
}
