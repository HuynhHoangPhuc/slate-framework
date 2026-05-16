//! Integration test for Image element surviving device-lost recovery.
//!
//! Validates that an Image element continues to render correctly after the
//! device-lost recovery state machine completes. Uses the RecoveryHarness
//! pattern with a custom View containing an Image.
//!
//! Windows-only with `test-hooks` feature (recovery harness requires Windows DXGI).

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::Cell;
use std::rc::Rc;
use std::thread;
use std::time::{Duration, Instant};

use slate_framework::app::AppContext;
use slate_framework::app_state::{AppSignal, AppState, RecoveryState};
use slate_framework::element::AnyElement;
use slate_framework::elements::{Div, Image};
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::view::{IntoAny, View};
use slate_platform::{
    DefaultPlatform, Event, Platform, Window, WindowOptions, WindowRenderDelegate, wake_run_loop,
};

const TICK_INTERVAL: Duration = Duration::from_millis(10);

/// Generate a 16x16 checkerboard pattern for testing.
fn generate_test_checkerboard() -> Vec<u8> {
    let mut pixels = Vec::with_capacity(16 * 16 * 4);
    for y in 0..16 {
        for x in 0..16 {
            let cell = ((x / 4) + (y / 4)) % 2 == 0;
            let v: u8 = if cell { 240 } else { 64 };
            pixels.extend_from_slice(&[v, v, v, 255]);
        }
    }
    pixels
}

/// View that renders a checkerboard Image.
struct ImageTestView {
    pixels: Vec<u8>,
}

impl ImageTestView {
    fn new() -> Self {
        Self {
            pixels: generate_test_checkerboard(),
        }
    }
}

impl View for ImageTestView {
    fn render(&mut self) -> AnyElement {
        Div::new()
            .child(Image::new(16, 16, self.pixels.clone()))
            .into_any()
    }
}

#[test]
fn image_survives_device_lost_recovery() {
    let _ = env_logger::builder().is_test(true).try_init();

    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-image-recovery-test".into(),
        size: (64, 64),
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

    let timeout = Duration::from_secs(5);
    let start = Instant::now();
    let initialized = Cell::new(false);
    let triggered = Cell::new(false);
    let initial_gen = Cell::new(0u64);
    let recovered = Cell::new(false);
    let frames_after_recovery = Cell::new(0u32);

    let mut view_factory = |_cx: &AppContext| ImageTestView::new();

    platform.run(|event| {
        if start.elapsed() > timeout {
            platform.quit();
            return;
        }

        if state.pending_quit() {
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
            Event::WindowResized { physical_size, .. } => {
                state.handle_window_resized(physical_size);
                false
            }
            Event::WindowCloseRequested { .. } | Event::WindowDestroyed { .. } => {
                platform.quit();
                return;
            }
            _ => false,
        };

        if !should_tick {
            return;
        }

        let sig = state.dispatch_redraw();
        if matches!(sig, AppSignal::RequestQuit) {
            platform.quit();
            return;
        }

        let recovery = state.current_recovery_state();
        let gen_now = state.renderer_generation().unwrap_or(0);

        // Trigger device-lost after initialization
        if initialized.get()
            && !triggered.get()
            && matches!(recovery, RecoveryState::NotLost)
            && !state.renderer_is_device_lost()
            && state.force_renderer_device_lost()
        {
            triggered.set(true);
            println!(
                "Device-lost triggered at gen={}, state={:?}",
                gen_now, recovery
            );
        }

        // Track recovery completion
        if triggered.get()
            && !recovered.get()
            && matches!(recovery, RecoveryState::NotLost)
            && gen_now > initial_gen.get()
        {
            recovered.set(true);
            println!(
                "Recovery completed: gen {} -> {}, state={:?}",
                initial_gen.get(),
                gen_now,
                recovery
            );
        }

        // Count frames after recovery to verify image still renders
        if recovered.get() {
            frames_after_recovery.set(frames_after_recovery.get() + 1);
            if frames_after_recovery.get() >= 3 {
                // Successfully rendered 3 frames after recovery
                platform.quit();
                return;
            }
        }

        // Give up if recovery failed
        if matches!(recovery, RecoveryState::GiveUp) {
            println!("Recovery gave up");
            platform.quit();
            return;
        }

        thread::sleep(TICK_INTERVAL);
        wake_run_loop();
    });

    // Assertions
    assert!(
        triggered.get(),
        "Device-lost was never triggered (test may have timed out before init)"
    );
    assert!(
        recovered.get(),
        "Recovery did not complete (state machine never returned to NotLost with higher generation)"
    );
    assert!(
        frames_after_recovery.get() >= 3,
        "Did not render enough frames after recovery: got {}, expected >= 3",
        frames_after_recovery.get()
    );

    let final_gen = state.renderer_generation().unwrap_or(0);
    assert!(
        final_gen > initial_gen.get(),
        "Renderer generation did not increment: {} -> {}",
        initial_gen.get(),
        final_gen
    );

    println!(
        "Image device-lost recovery test passed: gen {} -> {}, {} frames after recovery",
        initial_gen.get(),
        final_gen,
        frames_after_recovery.get()
    );
}
