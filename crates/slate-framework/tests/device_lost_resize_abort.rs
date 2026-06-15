//! Integration test: after a device-lost signal, a (non-live) window resize
//! issues NO GPU work — the configured surface size does not advance and the
//! process does not crash.
//!
//! Locks the acceptance criterion "`Renderer::resize` never calls
//! `queue.write_buffer` after configure marks the device lost." A removed
//! device that still received a viewport `queue.write_buffer` would create a
//! wgpu staging buffer on a dead device — the exact
//! `Placed buffer creation failed 0x887A0005` crash. The black-box observable
//! is the renderer's configured surface size: it must stay at the baseline
//! after a resize requested while the device is lost.
//!
//! Synthetic loss via `fire_renderer_device_lost_callback` keeps the GPU usable
//! so the assertion is deterministic (no real `RemoveDevice()` flakiness).
//! Windows + test-hooks gated; empty elsewhere.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::Cell;
use std::rc::Rc;

use slate_framework::app::AppContext;
use slate_framework::app_state::window_state::WindowState;
use slate_framework::app_state::AppState;
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::erased_view::ErasedView;
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::view::{IntoAny, View};
use slate_platform::{
    DefaultPlatform, Event, Platform, Window, WindowOptions, WindowRenderDelegate, wake_run_loop,
};

// Distinct from any plausible 1x1 init size so the "size did not advance"
// assertion is unambiguous.
const RESIZE_AFTER_LOSS: (u32, u32) = (640, 480);

struct NoopView;
impl View for NoopView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new().into_any()
    }
}

#[test]
fn resize_after_device_lost_issues_no_gpu_work() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-resize-abort".into(),
        size: (1, 1),
        min_size: None,
        resizable: true,
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

    let done = Cell::new(false);
    let baseline_size = Cell::new((0u32, 0u32));
    let size_after_resize = Cell::new((0u32, 0u32));
    let still_device_lost = Cell::new(false);
    let mut view_factory = |_cx: &AppContext| Box::new(NoopView) as Box<dyn ErasedView>;

    platform.run(|event| {
        if !matches!(event, Event::Resumed) {
            return;
        }
        if state
            .init_surfaces(window_id, &mut view_factory, &cx, &platform)
            .is_err()
        {
            platform.quit();
            return;
        }
        // One clean frame establishes the baseline surface size.
        state.dispatch_redraw(window_id);
        baseline_size.set(
            state
                .renderer_surface_size_for_test(window_id)
                .unwrap_or((0, 0)),
        );

        // Synthetic device loss — flag set, GPU still alive.
        assert!(
            state.fire_renderer_device_lost_callback(
                wgpu::DeviceLostReason::Unknown,
                "synthetic loss before resize".to_owned(),
            ),
            "renderer not initialized"
        );

        // Non-live resize while the device is lost. Must early-return inside
        // `Renderer::resize` BEFORE any configure / viewport write_buffer, so
        // the configured surface size does not advance and nothing crashes.
        state.handle_window_resized(window_id, RESIZE_AFTER_LOSS);
        size_after_resize.set(
            state
                .renderer_surface_size_for_test(window_id)
                .unwrap_or((0, 0)),
        );
        still_device_lost.set(state.renderer_is_device_lost_for(window_id));

        done.set(true);
        platform.quit();
    });

    assert!(done.get(), "test loop did not run to completion");
    assert_ne!(
        baseline_size.get(),
        RESIZE_AFTER_LOSS,
        "baseline must differ from the post-loss resize target for an unambiguous assertion"
    );
    assert_eq!(
        size_after_resize.get(),
        baseline_size.get(),
        "resize while device-lost must NOT reconfigure the surface (no GPU work); \
         size advanced from {:?} to {:?}",
        baseline_size.get(),
        size_after_resize.get(),
    );
    assert!(
        still_device_lost.get(),
        "device should remain marked lost (recoverable) after the aborted resize"
    );
}
