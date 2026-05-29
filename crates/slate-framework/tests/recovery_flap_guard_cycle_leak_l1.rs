//! L1 regression test (own file due to recovery state pollution): after a
//! `LuidMigration` recovery, the renderer's `wgpu_callback_fired` flag must
//! be clear so the next `LuidMigration` classifies correctly (and not as
//! `WgpuCallback`).
//!
//! Recovery completion is detected via the `Recovered { at }` state
//! timestamp — the renderer's per-instance generation counter resets on
//! every rebuild, so it can't track multi-cycle progress.

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
fn l1_cycle_leak_regression_luid_after_luid() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-l1-cycle-leak".into(),
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
    let first_fired = Cell::new(false);
    let second_fired = Cell::new(false);
    let first_fire_at = Cell::new(None::<Instant>);
    let second_fire_at = Cell::new(None::<Instant>);
    let last_recovered_at = Cell::new(None::<Instant>);
    let observed_state_after_second = Cell::new(None::<RecoveryState>);
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

        if let RecoveryState::Recovered { at } = state.current_recovery_state() {
            last_recovered_at.set(Some(at));
        }

        if initialized.get() && !first_fired.get() {
            state.force_renderer_device_lost_luid_migration_for_test();
            first_fire_at.set(Some(Instant::now()));
            first_fired.set(true);
        } else if first_fired.get() && !second_fired.get() {
            let first_recovered = last_recovered_at
                .get()
                .zip(first_fire_at.get())
                .map(|(rec, fire)| rec > fire)
                .unwrap_or(false);
            if first_recovered {
                assert!(
                    !state.consume_wgpu_callback_fired_for_test(),
                    "wgpu_callback_fired must be clear after LuidMigration recovery"
                );
                state.force_renderer_device_lost_luid_migration_for_test();
                second_fire_at.set(Some(Instant::now()));
                second_fired.set(true);
            }
        } else if second_fired.get() {
            let s = state.current_recovery_state();
            let second_recovered = last_recovered_at
                .get()
                .zip(second_fire_at.get())
                .map(|(rec, fire)| rec > fire)
                .unwrap_or(false);
            match s {
                _ if second_recovered => {
                    observed_state_after_second.set(Some(RecoveryState::NotLost));
                    platform.quit();
                    return;
                }
                RecoveryState::GiveUp { .. } => {
                    observed_state_after_second.set(Some(s));
                    platform.quit();
                    return;
                }
                _ => {}
            }
        }

        thread::sleep(TICK_INTERVAL);
        wake_run_loop();
    });

    assert!(
        first_fired.get() && second_fired.get(),
        "did not fire both losses"
    );
    let observed = observed_state_after_second.into_inner();
    assert!(
        matches!(observed, Some(RecoveryState::NotLost)),
        "cycle-leak regression: 2nd LuidMigration after LuidMigration must classify as LuidMigration (no GiveUp), got {:?}",
        observed
    );
}
