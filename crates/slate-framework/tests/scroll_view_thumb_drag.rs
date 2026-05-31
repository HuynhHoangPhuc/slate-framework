//! End-to-end scrollbar-thumb drag test.
//!
//! Drives the **production** thumb handlers (`build_thumb_handlers_for_test`)
//! through the real `AppState::dispatch_mouse_*` paths: mouse-down on the thumb
//! auto-captures it, subsequent moves route to the captured element and convert
//! pointer travel into a new scroll offset, and mouse-up ends the drag. Proves
//! the wiring the headless paint tests can't reach (no dispatch) and the unit
//! tests cover only as pure math.
//!
//! `harness = false`: its `fn main` runs on the process main thread so the real
//! platform + 1×1 window construct on macOS. `required-features =
//! ["test-hooks"]` gates the build on the `_for_test` hooks.

use std::rc::Rc;

macro_rules! ensure {
    ($cond:expr, $($arg:tt)*) => {
        match $cond {
            true => {}
            false => return Err(format!($($arg)*)),
        }
    };
}

use slate_framework::app_state::AppState;
use slate_framework::app_state::window_state::WindowState;
use slate_framework::elements::scroll_view::build_thumb_handlers_for_test;
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::hit_test::HitRegion;
use slate_framework::reactive::Signal;
use slate_framework::types::{Bounds, ElementId, Point, Size};
use slate_framework::{Modifiers, MouseButton};
use slate_platform::{DefaultPlatform, Platform, Window, WindowId, WindowOptions, wake_run_loop};

fn make_state() -> (Rc<AppState>, WindowId, std::sync::Arc<slate_reactive::Runtime>) {
    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-thumb-drag-test".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32000, -32000)),
    });
    let redraw_requester = RedrawRequester::new(wake_run_loop);
    let executor = Executor::new(redraw_requester.clone());
    let runtime = slate_reactive::Runtime::new();
    let _ = platform;
    let state = Rc::new(AppState::new(
        executor,
        redraw_requester.clone(),
        runtime.clone(),
    ));
    let window_id = window.id();
    {
        let win_state = WindowState::new(window, runtime.clone());
        state.windows.borrow_mut().insert(window_id, win_state);
    }
    state.register_redraw_requester_for_test(window_id, redraw_requester);
    (state, window_id, runtime)
}

/// Wire the thumb at id 1 with a 8×48 hit region near the top of a track, and
/// the production drag handlers (max_offset 96, track_travel 48 → 2 content px
/// per drag px).
fn check_thumb_drag_updates_offset() -> Result<(), String> {
    let (state, win, rt) = make_state();
    let thumb = ElementId::from_raw(1);
    let offset = Signal::new(rt, 0.0_f32);

    state.install_element_mouse_handlers_for_test(
        win,
        thumb,
        build_thumb_handlers_for_test(offset.clone(), 96.0, 48.0),
    );
    state.push_hit_region_for_test(
        win,
        HitRegion::new(
            thumb,
            Bounds {
                origin: Point::new(110.0, 2.0),
                size: Size::new(8.0, 48.0),
            },
            0,
        ),
    );

    // Press the thumb at y=10 (anchors the drag, auto-captures).
    state.dispatch_mouse_down_for_test(win, (114.0, 10.0), MouseButton::Left, Modifiers::default());
    ensure!(
        state.capture_target_for_test(win) == Some(thumb),
        "thumb auto-captured on mouse-down"
    );

    // Drag down 10px → offset moves by 10 * (96/48) = 20.
    state.dispatch_mouse_move_for_test(win, (114.0, 20.0));
    ensure!(
        (offset.get() - 20.0).abs() < 1e-3,
        "drag converts pointer travel to offset (got {})",
        offset.get()
    );

    // Drag past the bottom → clamps at max_offset 96.
    state.dispatch_mouse_move_for_test(win, (114.0, 500.0));
    ensure!(
        (offset.get() - 96.0).abs() < 1e-3,
        "offset clamps to max (got {})",
        offset.get()
    );

    // Release → auto-capture cleared (all buttons up, not explicit).
    state.dispatch_mouse_up_for_test(win, (114.0, 500.0), MouseButton::Left, Modifiers::default());
    ensure!(
        state.capture_target_for_test(win).is_none(),
        "auto-capture released on mouse-up"
    );

    // A move after release must not change the offset (drag ended).
    let after = offset.get();
    state.dispatch_mouse_move_for_test(win, (114.0, 30.0));
    ensure!(
        (offset.get() - after).abs() < 1e-3,
        "post-release move is inert (got {})",
        offset.get()
    );

    Ok(())
}

type Case = (&'static str, fn() -> Result<(), String>);

fn main() {
    let cases: &[Case] = &[("thumb_drag_updates_offset", check_thumb_drag_updates_offset)];
    let mut failed = 0;
    for (name, f) in cases {
        match f() {
            Ok(()) => println!("test {name} ... ok"),
            Err(e) => {
                failed += 1;
                println!("test {name} ... FAILED\n    {e}");
            }
        }
    }
    if failed > 0 {
        eprintln!("\n{failed} test(s) failed");
        std::process::exit(1);
    }
    println!("\nall scroll_view_thumb_drag cases passed");
}
