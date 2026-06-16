//! Integration test: live-resize swapchain defer invariant.
//!
//! During a modal size/move drag (`is_live_resizing()` true), the swapchain
//! must NOT be reconfigured at all — not per `WindowResized`, and not even on
//! the timer-pumped redraws that fire mid-drag. The real `ResizeBuffers` is
//! deferred to size-move end; meanwhile the existing buffers stretch
//! (`DXGI_SCALING_STRETCH`) and layout reflows via a cheap viewport-uniform
//! update. Reconfiguring per drag frame storms `ResizeBuffers`, which suspends
//! the device (`0x887A0005`) mid-drag (plan 260615-1516, branch 2b).
//!
//! Observable = the renderer's configured surface size (black-box; no call
//! counter). Deferred ⇒ size stays at the pre-drag value through every stash
//! AND every redraw, until `on_size_move_end` applies the final size once.
//!
//! Predicate alignment: `DefaultWindow::is_live_resizing()` reads the same
//! `in_size_move` flag that `set_in_size_move_for_test` sets
//! (`slate-platform/src/win/window.rs`), so the test drives the exact gate the
//! production defer checks.
//!
//! Standard `#[test]` harness like the `recovery_size_move_*` siblings; the
//! cfg gate makes it empty off Windows / without test-hooks.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::Cell;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

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

const TICK_INTERVAL: Duration = Duration::from_millis(10);
const HARD_TIMEOUT: Duration = Duration::from_secs(20);

// Distinct from each other and from any plausible 1x1-window init size, so the
// deferred/applied assertions are unambiguous.
const DRAG_A: (u32, u32) = (200, 150);
const DRAG_B: (u32, u32) = (300, 220);
const DRAG_C: (u32, u32) = (400, 300); // last live-drag size → applied on size-move end

struct NoopView;
impl View for NoopView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new().into_any()
    }
}

#[test]
fn live_drag_defers_resizebuffers_until_size_move_end() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-resize-coalesce".into(),
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
    // Step machine: 0 = await init, 1 = stash-during-drag, 2 = redraw-during-drag,
    // 3 = size-move-end-applies, 4 = done.
    let step = Cell::new(0u32);
    let init_size = Cell::new((0u32, 0u32));
    let deferred_size = Cell::new((0u32, 0u32));
    let drag_redraw_size = Cell::new((0u32, 0u32));
    let applied_size = Cell::new((0u32, 0u32));
    let done = Cell::new(false);
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
                // Baseline surface size after init + one redraw.
                state.dispatch_redraw(window_id);
                init_size.set(
                    state
                        .renderer_surface_size_for_test(window_id)
                        .unwrap_or((0, 0)),
                );
                true
            }
            Event::Wake | Event::WindowRedrawRequested { .. } => true,
            _ => false,
        };
        if !should_tick {
            return;
        }

        match step.get() {
            0 => {
                // Live drag: stash three increasing sizes with NO redraw between.
                window.set_in_size_move_for_test(true);
                state.handle_window_resized(window_id, DRAG_A);
                state.handle_window_resized(window_id, DRAG_B);
                state.handle_window_resized(window_id, DRAG_C);
                deferred_size.set(
                    state
                        .renderer_surface_size_for_test(window_id)
                        .unwrap_or((0, 0)),
                );
                step.set(1);
            }
            1 => {
                // A redraw DURING the drag must NOT reconfigure the swapchain:
                // ResizeBuffers stays deferred (DXGI stretch covers the gap),
                // so the surface size is still the pre-drag value.
                state.dispatch_redraw(window_id);
                drag_redraw_size.set(
                    state
                        .renderer_surface_size_for_test(window_id)
                        .unwrap_or((0, 0)),
                );
                step.set(2);
            }
            2 => {
                // Modal loop ends: exactly one reconfigure lands at the final
                // (last live-drag) size.
                window.set_in_size_move_for_test(false);
                state.on_size_move_end(window_id);
                applied_size.set(
                    state
                        .renderer_surface_size_for_test(window_id)
                        .unwrap_or((0, 0)),
                );
                step.set(3);
                done.set(true);
                platform.quit();
                return;
            }
            _ => {
                platform.quit();
                return;
            }
        }

        thread::sleep(TICK_INTERVAL);
        wake_run_loop();
    });

    assert!(done.get(), "test loop did not complete all steps");

    // Deferred: surface size must NOT change while the drag stashes sizes.
    assert_eq!(
        deferred_size.get(),
        init_size.get(),
        "live-drag resizes must be deferred (surface stayed {:?}), but it changed to {:?} \
         — per-WM_SIZE ResizeBuffers not deferred",
        init_size.get(),
        deferred_size.get(),
    );

    // Deferred through redraw: a redraw mid-drag must STILL not reconfigure —
    // the swapchain churn is what suspends the device, so it must reach zero
    // during the drag, not merely coalesce to one-per-redraw.
    assert_eq!(
        drag_redraw_size.get(),
        init_size.get(),
        "a redraw during in_size_move must NOT reconfigure the swapchain \
         (surface stayed {:?}), but it changed to {:?} — ResizeBuffers ran mid-drag",
        init_size.get(),
        drag_redraw_size.get(),
    );

    // Applied once on size-move end at exactly the last requested size.
    assert_eq!(
        applied_size.get(),
        DRAG_C,
        "on_size_move_end must apply the final live-drag size {:?} exactly once, got {:?}",
        DRAG_C,
        applied_size.get(),
    );
}
