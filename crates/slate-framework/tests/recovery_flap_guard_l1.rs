//! L1 integration test: three `LuidMigration` losses within 1 second must
//! never trip the flap guard — legitimate cross-adapter migration during a
//! drag-across-the-seam must not be misclassified as a faulty GPU.
//!
//! Recovery completion is detected via the `Recovered { at }` state's
//! timestamp advancing past the last-fire instant — the renderer's
//! per-instance generation counter resets on every rebuild, so it can't
//! track multi-cycle progress.
//!
//! Lives in its own file: running multiple full-recovery scenarios in one
//! test binary leaves wgpu/DXGI state that blocks the next platform/window
//! from completing recovery.

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
fn l1_luid_migration_never_gives_up() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-l1-luid-migration".into(),
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
    let losses_fired = Cell::new(0u32);
    let last_fire_at = Cell::new(Instant::now());
    let last_recovered_at = Cell::new(None::<Instant>);
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
                initialized.set(true);
                true
            }
            Event::Wake | Event::WindowRedrawRequested { .. } => true,
            _ => false,
        };
        if !should_tick {
            return;
        }

        state.dispatch_redraw(state.window_id_for_test());

        // GiveUp at any point is failure.
        if matches!(state.current_recovery_state(), RecoveryState::GiveUp { .. }) {
            platform.quit();
            return;
        }

        // Capture each `Recovered { at }` transition as it arrives.
        if let RecoveryState::Recovered { at } = state.current_recovery_state() {
            last_recovered_at.set(Some(at));
        }

        // Fire next loss once previous one has recovered (its `at` instant is
        // strictly after the last fire).
        if initialized.get() && losses_fired.get() < 3 {
            let first = losses_fired.get() == 0;
            let recovered_after_last_fire = last_recovered_at
                .get()
                .map(|at| at > last_fire_at.get())
                .unwrap_or(false);
            if first || recovered_after_last_fire {
                state.force_renderer_device_lost_luid_migration_for_test();
                last_fire_at.set(Instant::now());
                losses_fired.set(losses_fired.get() + 1);
            }
        }

        // Done once all 3 have fired AND the last recovery's `at` post-dates
        // the final fire.
        if losses_fired.get() == 3
            && last_recovered_at
                .get()
                .map(|at| at > last_fire_at.get())
                .unwrap_or(false)
        {
            platform.quit();
            return;
        }

        thread::sleep(TICK_INTERVAL);
        wake_run_loop();
    });

    assert_eq!(
        losses_fired.get(),
        3,
        "did not fire all 3 LuidMigration losses"
    );
    assert!(
        !matches!(state.current_recovery_state(), RecoveryState::GiveUp { .. }),
        "L1: 3× LuidMigration must NOT GiveUp, got {:?}",
        state.current_recovery_state()
    );
    assert!(
        last_recovered_at
            .get()
            .map(|at| at > last_fire_at.get())
            .unwrap_or(false),
        "L1: final LuidMigration loss never reached Recovered (last_recovered_at={:?}, last_fire_at={:?})",
        last_recovered_at.get(),
        last_fire_at.get()
    );
}
