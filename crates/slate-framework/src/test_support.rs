//! Test-only harness for exercising the device-lost recovery state machine
//! end-to-end on a real off-screen DXGI surface.
//!
//! Gated on the `test-hooks` feature. The harness creates a real platform
//! window (Windows: `WS_POPUP | WS_DISABLED | WS_VISIBLE` at `(-32000, -32000)`,
//! 1×1) so `Renderer::force_device_lost` has a real DXGI swap chain to operate
//! on, while keeping the window outside the user's desktop.
//!
//! The harness drives the redraw state machine via `Event::Wake` ticks rather
//! than `WM_PAINT`: an off-screen window's update region is empty, so the OS
//! does not reliably synthesize `WM_PAINT` after `InvalidateRect`. A wake-based
//! tick is independent of any compositor scheduling and works for both visible
//! and off-screen test windows.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use slate_platform::{
    DefaultPlatform, DefaultWindow, Event, Platform, Window, WindowOptions, WindowRenderDelegate,
    wake_run_loop,
};

use crate::app::AppContext;
use crate::app_state::{AppSignal, AppState, RecoveryState};
use crate::element::AnyElement;
use crate::elements::Div;
use crate::executor::{Executor, RedrawRequester};
use crate::view::{IntoAny, View};

/// Tick interval between wake-driven redraw dispatches. Chosen to amortise
/// the 350ms recovery cooldown across ~35 cheap loop iterations without
/// burning CPU.
const TICK_INTERVAL: Duration = Duration::from_millis(10);

/// Minimal `View` that renders an empty `Div`. Used by the recovery harness so
/// the layout/paint pipeline runs without depending on a real UI tree.
pub struct NoopView;

impl View for NoopView {
    fn render(&mut self) -> AnyElement {
        Div::new().into_any()
    }
}

/// Result of `RecoveryHarness::probe_force_device_lost`.
#[derive(Debug, Clone)]
pub struct RecoveryProbe {
    pub initial_generation: u64,
    pub final_generation: u64,
    pub returned_to_not_lost: bool,
    pub gave_up: bool,
    pub timed_out_before_trigger: bool,
    pub timed_out_in_recovery: bool,
    pub request_quit: bool,
    pub elapsed: Duration,
    pub final_state: RecoveryState,
}

/// Real-platform off-screen recovery harness.
pub struct RecoveryHarness {
    platform: DefaultPlatform,
    #[allow(dead_code)]
    window: Arc<DefaultWindow>,
    state: Rc<AppState<NoopView>>,
    cx: AppContext,
}

impl RecoveryHarness {
    pub fn new() -> Self {
        let platform = DefaultPlatform::new();
        let window = platform.create_window(WindowOptions {
            title: "slate-recovery-harness".into(),
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

        Self {
            platform,
            window,
            state,
            cx,
        }
    }

    /// Run the platform event loop, ticking the redraw state machine via wake
    /// events. After the first clean frame, fires `force_device_lost` and
    /// observes the state machine through to `NotLost` with an incremented
    /// renderer generation, or to `GiveUp`, or to `timeout`.
    pub fn probe_force_device_lost(self, timeout: Duration) -> RecoveryProbe {
        let start = Instant::now();
        let triggered = Cell::new(false);
        let initial_gen = Cell::new(0u64);
        let request_quit = Cell::new(false);
        let mut view_factory = |_cx: &AppContext| NoopView;

        let RecoveryHarness {
            platform,
            window: _window,
            state,
            cx,
        } = &self;

        platform.run(|event| {
            if start.elapsed() > timeout {
                platform.quit();
                return;
            }

            if state.pending_quit() {
                request_quit.set(true);
                platform.quit();
                return;
            }

            let should_tick = match event {
                Event::Resumed => {
                    if state
                        .init_surfaces(&mut view_factory, cx, platform)
                        .is_err()
                    {
                        request_quit.set(true);
                        platform.quit();
                        return;
                    }
                    initial_gen.set(state.renderer_generation().unwrap_or(0));
                    true
                }
                Event::Wake => true,
                Event::WindowRedrawRequested { .. } => true,
                Event::WindowResized { physical_size, .. } => {
                    state.handle_window_resized(physical_size);
                    false
                }
                Event::WindowCloseRequested { .. } => {
                    request_quit.set(true);
                    platform.quit();
                    return;
                }
                Event::WindowDestroyed { .. } => {
                    if matches!(state.handle_window_destroyed(), AppSignal::RequestQuit) {
                        request_quit.set(true);
                        platform.quit();
                    }
                    return;
                }
                _ => false,
            };

            if !should_tick {
                return;
            }

            let sig = state.dispatch_redraw();
            if matches!(sig, AppSignal::RequestQuit) {
                request_quit.set(true);
                platform.quit();
                return;
            }

            let recovery = state.current_recovery_state();
            let gen_now = state.renderer_generation().unwrap_or(0);

            if !triggered.get()
                && matches!(recovery, RecoveryState::NotLost)
                && gen_now >= initial_gen.get()
                && !state.renderer_is_device_lost()
            {
                // Module is gated on Windows + test-hooks, so force_renderer_device_lost
                // is always available here.
                if state.force_renderer_device_lost() {
                    triggered.set(true);
                }
                schedule_next_tick();
            } else if (triggered.get()
                && matches!(recovery, RecoveryState::NotLost)
                && gen_now > initial_gen.get())
                || matches!(recovery, RecoveryState::GiveUp { .. })
            {
                platform.quit();
            } else {
                schedule_next_tick();
            }
        });

        let final_gen = state.renderer_generation().unwrap_or(0);
        let final_state = state.current_recovery_state();
        let elapsed = start.elapsed();

        let triggered_b = triggered.get();
        let returned_to_not_lost = triggered_b
            && matches!(final_state, RecoveryState::NotLost)
            && final_gen > initial_gen.get();
        let gave_up = matches!(final_state, RecoveryState::GiveUp { .. });
        let timed_out_before_trigger = !triggered_b && elapsed >= timeout;
        let timed_out_in_recovery =
            triggered_b && elapsed >= timeout && !returned_to_not_lost && !gave_up;

        RecoveryProbe {
            initial_generation: initial_gen.get(),
            final_generation: final_gen,
            returned_to_not_lost,
            gave_up,
            timed_out_before_trigger,
            timed_out_in_recovery,
            request_quit: request_quit.get(),
            elapsed,
            final_state,
        }
    }
}

impl Default for RecoveryHarness {
    fn default() -> Self {
        Self::new()
    }
}

fn schedule_next_tick() {
    thread::sleep(TICK_INTERVAL);
    wake_run_loop();
}
