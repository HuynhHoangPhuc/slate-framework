//! L1 integration test (own file): `LuidMigration` then `WgpuCallback`
//! upgrades the stored reason; a follow-up `WgpuCallback` within 5s then
//! triggers `GiveUp { reason: WgpuCallback }`.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::Cell;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use slate_framework::DeviceLossReason;
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
fn l1_upgrade_then_flap_gives_up() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-l1-upgrade-flap".into(),
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
    let phase = Cell::new(0u32); // 0=before first, 1=fired LUID, 2=fired callback, 3=fired second callback
    let initial_gen = Cell::new(0u64);
    let gen_after_first_recovery = Cell::new(0u64);
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
                true
            }
            Event::Wake | Event::WindowRedrawRequested { .. } => true,
            _ => false,
        };
        if !should_tick {
            return;
        }

        state.dispatch_redraw();

        match phase.get() {
            0 if initialized.get() => {
                state.force_renderer_device_lost_luid_migration_for_test();
                phase.set(1);
            }
            1 => {
                // Upgrade: fire wgpu callback while still in recovery. The
                // deferred upgrade rule replaces `LuidMigration` with
                // `WgpuCallback` and stamps `last_wgpu_callback_loss_at`.
                state.fire_renderer_device_lost_callback(
                    wgpu::DeviceLostReason::Unknown,
                    "synthetic upgrade".into(),
                );
                phase.set(2);
            }
            2 if matches!(state.current_recovery_state(), RecoveryState::NotLost)
                && state.renderer_generation().unwrap_or(0) > initial_gen.get() =>
            {
                gen_after_first_recovery.set(state.renderer_generation().unwrap_or(0));
                state.fire_renderer_device_lost_callback(
                    wgpu::DeviceLostReason::Unknown,
                    "synthetic second callback".into(),
                );
                phase.set(3);
            }
            _ => {}
        }

        if matches!(state.current_recovery_state(), RecoveryState::GiveUp { .. }) {
            platform.quit();
            return;
        }

        thread::sleep(TICK_INTERVAL);
        wake_run_loop();
    });

    let final_state = state.current_recovery_state();
    assert_eq!(phase.get(), 3, "did not reach phase=3 (all 3 losses fired)");
    match final_state {
        RecoveryState::GiveUp { reason } => assert_eq!(
            reason,
            DeviceLossReason::WgpuCallback,
            "upgrade-then-flap GiveUp reason must be WgpuCallback"
        ),
        other => panic!("L1 upgrade+flap must GiveUp{{WgpuCallback}}, got {:?}", other),
    }
}
