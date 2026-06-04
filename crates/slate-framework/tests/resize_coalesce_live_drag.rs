//! Integration test: live-resize swapchain coalescing invariant.
//!
//! During a modal size/move drag (`is_live_resizing()` true), repeated
//! `WindowResized` events must NOT reconfigure the swapchain per event — the
//! latest size is stashed and applied exactly once on the next timer-pumped
//! redraw. Resizing per WM_SIZE storms `ResizeBuffers` (repeated failures on
//! wgpu+DComp escalate to `0x887A0005` device-removed) and paints a stale
//! framebuffer at the new bounds (blink).
//!
//! Observable = the renderer's configured surface size (black-box; no call
//! counter). Deferred ⇒ size unchanged until redraw; coalesced ⇒ applied once.
//!
//! Predicate alignment: `DefaultWindow::is_live_resizing()` reads the same
//! `in_size_move` flag that `set_in_size_move_for_test` sets
//! (`slate-platform/src/win/window.rs`), so the test drives the exact gate the
//! production fix checks.
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
// deferred/coalesced/immediate assertions are unambiguous.
const DRAG_A: (u32, u32) = (200, 150);
const DRAG_B: (u32, u32) = (300, 220);
const DRAG_C: (u32, u32) = (400, 300); // last live-drag size → coalesced apply
const NON_LIVE: (u32, u32) = (250, 180); // post-drag, applies immediately

struct NoopView;
impl View for NoopView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new().into_any()
    }
}

#[test]
fn live_drag_resizes_coalesce_to_one_apply_on_redraw() {
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
    // Step machine: 0 = await init, 1 = stash-during-drag, 2 = redraw-coalesce,
    // 3 = non-live-immediate, 4 = done.
    let step = Cell::new(0u32);
    let init_size = Cell::new((0u32, 0u32));
    let deferred_size = Cell::new((0u32, 0u32));
    let coalesced_size = Cell::new((0u32, 0u32));
    let immediate_size = Cell::new((0u32, 0u32));
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
                // One redraw applies the latest stashed size exactly once.
                state.dispatch_redraw(window_id);
                coalesced_size.set(
                    state
                        .renderer_surface_size_for_test(window_id)
                        .unwrap_or((0, 0)),
                );
                step.set(2);
            }
            2 => {
                // Drag ended: a trailing resize applies immediately.
                window.set_in_size_move_for_test(false);
                state.handle_window_resized(window_id, NON_LIVE);
                immediate_size.set(
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

    // Deferred: surface size must NOT change while the drag is in flight.
    assert_eq!(
        deferred_size.get(),
        init_size.get(),
        "live-drag resizes must be deferred (surface stayed {:?}), but it changed to {:?} \
         — per-WM_SIZE ResizeBuffers not coalesced",
        init_size.get(),
        deferred_size.get(),
    );

    // Coalesced: redraw applies exactly the LAST requested size.
    assert_eq!(
        coalesced_size.get(),
        DRAG_C,
        "redraw must coalesce to the last live-drag size {:?}, got {:?}",
        DRAG_C,
        coalesced_size.get(),
    );

    // Non-live: a resize outside the drag applies immediately.
    assert_eq!(
        immediate_size.get(),
        NON_LIVE,
        "non-live resize must apply immediately ({:?}), got {:?}",
        NON_LIVE,
        immediate_size.get(),
    );
}
