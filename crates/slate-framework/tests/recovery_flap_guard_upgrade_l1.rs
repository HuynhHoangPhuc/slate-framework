//! L1 integration test (own file): a `LuidMigration` loss whose reason is
//! upgraded to `WgpuCallback` mid-cycle, followed by another `WgpuCallback`
//! within 5s, must RECOVER — not quit.
//!
//! This still exercises the deferred upgrade rule (`maybe_upgrade_reason`
//! replaces `LuidMigration` with `WgpuCallback` and stamps
//! `last_wgpu_callback_loss_at`). What changed under wait-until-healthy is the
//! terminal outcome: the follow-up WgpuCallback inside the flap window used to
//! escalate to `GiveUp { WgpuCallback }`; it now keeps probing and recovers.
//! The flap window is telemetry only — the sole quit path is the wall-clock
//! give-up budget after continuous failure, which a recovering flap never hits.
//!
//! Recovery completion is detected via the `Recovered { at }` state's
//! timestamp advancing past each fire instant — the renderer's per-instance
//! generation counter resets to 1 on every rebuild, so it cannot track
//! multi-cycle progress.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::Cell;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use slate_framework::app::AppContext;
use slate_framework::app_state::window_state::WindowState;
use slate_framework::app_state::{AppState, RecoveryState};
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
fn l1_upgrade_then_flap_recovers_not_quits() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-l1-upgrade-flap".into(),
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
    {
        state
            .windows
            .borrow_mut()
            .insert(window_id, WindowState::new(window.clone(), runtime));
    }
    state.register_redraw_requester_for_test(window_id, redraw_requester);

    let dyn_strong: Rc<dyn WindowRenderDelegate> = state.clone();
    let dyn_weak = Rc::downgrade(&dyn_strong);
    window.set_render_delegate(dyn_weak);
    drop(dyn_strong);

    let start = Instant::now();
    let initialized = Cell::new(false);
    // 0=before first, 1=fired LUID, 2=fired upgrade callback, 3=fired 2nd callback
    let phase = Cell::new(0u32);
    let episode1_fire_at = Cell::new(Instant::now());
    let episode2_fire_at = Cell::new(Instant::now());
    let last_recovered_at = Cell::new(None::<Instant>);
    let gave_up = Cell::new(false);
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

        // A flap re-fire must NEVER quit under wait-until-healthy.
        if matches!(state.current_recovery_state(), RecoveryState::GiveUp { .. }) {
            gave_up.set(true);
            platform.quit();
            return;
        }

        // Capture each `Recovered { at }` transition as it arrives.
        if let RecoveryState::Recovered { at } = state.current_recovery_state() {
            last_recovered_at.set(Some(at));
        }

        let recovered_after = |fire: Instant| {
            last_recovered_at
                .get()
                .map(|at| at > fire)
                .unwrap_or(false)
        };

        match phase.get() {
            0 if initialized.get() => {
                state.force_renderer_device_lost_luid_migration_for_test();
                episode1_fire_at.set(Instant::now());
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
            2 if recovered_after(episode1_fire_at.get()) => {
                // Episode 1 recovered. Fire the second WgpuCallback inside the
                // 5s flap window — the case that used to GiveUp.
                state.fire_renderer_device_lost_callback(
                    wgpu::DeviceLostReason::Unknown,
                    "synthetic second callback".into(),
                );
                episode2_fire_at.set(Instant::now());
                phase.set(3);
            }
            3 if recovered_after(episode2_fire_at.get()) => {
                // Second (flapping) loss recovered too — done.
                platform.quit();
                return;
            }
            _ => {}
        }

        thread::sleep(TICK_INTERVAL);
        wake_run_loop();
    });

    assert_eq!(phase.get(), 3, "did not reach phase=3 (all 3 losses fired)");
    assert!(
        !gave_up.get(),
        "L1: upgrade-then-flap must NOT quit under wait-until-healthy"
    );
    assert!(
        last_recovered_at
            .get()
            .map(|at| at > episode2_fire_at.get())
            .unwrap_or(false),
        "L1: second (flapping) loss never reached Recovered (last_recovered_at={:?}, episode2_fire_at={:?})",
        last_recovered_at.get(),
        episode2_fire_at.get()
    );
}
