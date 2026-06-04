//! Integration test: a persistent adapter-LUID mismatch must escalate to the
//! device-lost recovery machine so the renderer rebuilds on the window's
//! current monitor's adapter.
//!
//! Regression lock for the cross-adapter crash: on a hybrid-GPU multi-monitor
//! laptop the window can end up on a monitor served by a different GPU than the
//! renderer's startup adapter. The per-redraw LUID probe detects this, but
//! previously escalated via `mark_device_potentially_lost()` — which consults
//! `GetDeviceRemovedReason()` and returns `S_OK` for a *healthy* device on the
//! *wrong* adapter, so it never marked the device lost. The renderer kept
//! cross-adapter DirectComposition-composing until DXGI hard-removed it
//! (`0x887A0005`) and the process died with no graceful recovery. The probe
//! must instead mark the device lost unconditionally on a confirmed mismatch.
//!
//! Deterministic + hardware-independent: both LUIDs are injected via test
//! hooks (`set_monitor_luid_for_test` on the window, `set_renderer_adapter_luid
//! _for_test` on the renderer), so no second physical GPU is required and the
//! real `GetDeviceRemovedReason()` on the healthy test device stays `S_OK` —
//! which is exactly the condition the old code mishandled.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::Cell;
use std::rc::Rc;
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

const HARD_TIMEOUT: Duration = Duration::from_secs(20);

// Two distinct injected LUIDs: window monitor vs renderer adapter.
const WINDOW_MONITOR_LUID: u64 = 0x0000_0000_AAAA_AAAA;
const RENDERER_ADAPTER_LUID: u64 = 0x0000_0000_BBBB_BBBB;

struct NoopView;
impl View for NoopView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new().into_any()
    }
}

#[test]
fn adapter_luid_mismatch_marks_device_lost_and_engages_recovery() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-luid-mismatch".into(),
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
    let probed = Cell::new(false);
    let device_lost_after = Cell::new(false);
    let recovery_engaged = Cell::new(false);
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
                // Inject a stable cross-adapter mismatch: the window reports it
                // sits on a monitor served by one GPU, the renderer reports it
                // was built on a different GPU. No second physical GPU needed;
                // the real device stays healthy (GetDeviceRemovedReason = S_OK),
                // which is the case the old escalation mishandled.
                window.set_monitor_luid_for_test(Some(WINDOW_MONITOR_LUID));
                state.set_renderer_adapter_luid_for_test(window_id, Some(RENDERER_ADAPTER_LUID));

                // One dispatch runs the per-redraw LUID probe. last_adapter_check_at
                // is None after init, so the probe is not throttled.
                state.dispatch_redraw(window_id);

                device_lost_after.set(state.renderer_is_device_lost_for(window_id));
                recovery_engaged
                    .set(!matches!(state.current_recovery_state(), RecoveryState::NotLost));
                probed.set(true);
                platform.quit();
            }
            _ => {}
        }
    });

    assert!(probed.get(), "probe never ran (Resumed not delivered)");

    // The confirmed mismatch must have marked the device lost...
    assert!(
        device_lost_after.get(),
        "adapter-LUID mismatch must mark the device lost so recovery rebuilds on \
         the window's adapter; device_lost stayed false (cross-adapter composition \
         would continue until DXGI hard-removes the device — 0x887A0005)"
    );
    // ...and the recovery state machine must have left NotLost.
    assert!(
        recovery_engaged.get(),
        "recovery must engage on adapter mismatch, but state is still NotLost"
    );
}
