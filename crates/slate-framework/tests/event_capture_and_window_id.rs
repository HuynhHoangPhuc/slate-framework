//! Integration tests for the `EventCtx` capture surface and the `window_id`
//! accessor threaded through dispatch.
//!
//! Drives the real `AppState` dispatch paths via the `*_for_test` hooks:
//!   - `set_capture` / `release_capture` queue a pending op honored after the
//!     handler chain unwinds (mirrors `request_focus`/`blur`).
//!   - explicit capture is sticky: it survives mouse-up so a handler can run a
//!     multi-step drag across button releases.
//!   - an unmounted captured element auto-releases (no stranded capture).
//!   - `EventCtx::window_id()` returns the originating window's id for both
//!     mouse and key dispatch.
//!
//! The "no public hooks API in v1" constraint is enforced structurally:
//! `use_state` lives on `StateRegistry`, whose module is `pub(crate)`, and
//! `RenderCx` exposes only `window_id()`. There is no public `use_state` to
//! exercise here.
//!
//! Runs cross-platform as a `harness = false` `[[test]]` (its `fn main`
//! executes on the process main thread, so the real platform + a 1×1 window
//! construct on macOS); `required-features = ["test-hooks"]` gates the build.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

macro_rules! ensure_eq {
    ($left:expr, $right:expr, $($arg:tt)*) => {{
        let left = $left;
        let right = $right;
        if left != right {
            return Err(format!(
                "{} (left: {:?}, right: {:?})",
                format!($($arg)*),
                left,
                right
            ));
        }
    }};
}

macro_rules! ensure {
    ($cond:expr, $($arg:tt)*) => {
        match $cond {
            true => {}
            false => return Err(format!($($arg)*)),
        }
    };
}

type Case = (&'static str, fn() -> Result<(), String>);

use slate_framework::app_state::AppState;
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::event::{EventCtx, MouseEvent, MouseHandlers};
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::hit_test::HitRegion;
use slate_framework::types::{Bounds, ElementId, Point, Size};
use slate_framework::view::{IntoAny, View};
use slate_framework::{Key, KeyCode, KeyEvent, Modifiers, MouseButton};
use slate_platform::{DefaultPlatform, Platform, WindowOptions, wake_run_loop};

struct NoopView;

impl View for NoopView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new().into_any()
    }
}

fn make_state() -> Rc<AppState<NoopView>> {
    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-capture-windowid-test".into(),
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
    Rc::new(AppState::new(window, executor, redraw_requester, runtime))
}

fn id(n: u64) -> ElementId {
    ElementId::from_raw(n)
}

fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds {
    Bounds {
        origin: Point::new(x, y),
        size: Size::new(w, h),
    }
}

type Mouse = Arc<dyn Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static>;

/// Install a hit region for `elem` and a down-handler that runs `f` with the
/// dispatch `EventCtx`.
fn wire_down_handler(state: &AppState<NoopView>, elem: ElementId, b: Bounds, f: Mouse) {
    state.install_element_mouse_handlers_for_test(
        elem,
        MouseHandlers {
            on_mouse_down: Some(f),
            on_mouse_move: None,
            on_mouse_up: None,
        },
    );
    state.hit_test_list.borrow_mut().push(HitRegion::new(elem, b, 0));
}

fn check_set_capture_overrides_and_marks_explicit() -> Result<(), String> {
    let state = make_state();
    let a = id(1);
    let b = id(99); // distinct id, no hit region — proves the op, not the auto-set
    wire_down_handler(
        &state,
        a,
        bounds(0.0, 0.0, 100.0, 100.0),
        Arc::new(move |_ev, cx| cx.set_capture(b)),
    );

    state.dispatch_mouse_down_for_test((10.0, 10.0), MouseButton::Left, Modifiers::default());

    ensure_eq!(
        *state.capture_target.borrow(),
        Some(b),
        "explicit set_capture overrides the mouse-down auto-capture"
    );
    ensure!(
        *state.explicit_capture.borrow(),
        "set_capture marks the capture explicit"
    );
    Ok(())
}

fn check_release_capture_clears_target() -> Result<(), String> {
    let state = make_state();
    let a = id(1);
    wire_down_handler(
        &state,
        a,
        bounds(0.0, 0.0, 100.0, 100.0),
        Arc::new(|_ev, cx| cx.release_capture()),
    );

    state.dispatch_mouse_down_for_test((10.0, 10.0), MouseButton::Left, Modifiers::default());

    ensure_eq!(
        *state.capture_target.borrow(),
        None,
        "release_capture clears the auto-set capture"
    );
    ensure!(
        !*state.explicit_capture.borrow(),
        "release_capture clears the explicit flag"
    );
    Ok(())
}

fn check_explicit_capture_survives_mouse_up() -> Result<(), String> {
    let state = make_state();
    let a = id(1);
    wire_down_handler(
        &state,
        a,
        bounds(0.0, 0.0, 100.0, 100.0),
        Arc::new(move |_ev, cx| cx.set_capture(a)),
    );

    state.dispatch_mouse_down_for_test((10.0, 10.0), MouseButton::Left, Modifiers::default());
    ensure_eq!(*state.capture_target.borrow(), Some(a), "captured on down");

    state.dispatch_mouse_up_for_test((10.0, 10.0), MouseButton::Left, Modifiers::default());
    ensure_eq!(
        *state.capture_target.borrow(),
        Some(a),
        "explicit capture is sticky across mouse-up (multi-step drag)"
    );
    Ok(())
}

fn check_unmounted_captured_element_auto_releases() -> Result<(), String> {
    let state = make_state();
    let a = id(1);
    wire_down_handler(
        &state,
        a,
        bounds(0.0, 0.0, 100.0, 100.0),
        Arc::new(move |_ev, cx| cx.set_capture(a)),
    );
    state.dispatch_mouse_down_for_test((10.0, 10.0), MouseButton::Left, Modifiers::default());
    ensure_eq!(*state.capture_target.borrow(), Some(a), "captured on down");

    // Still mounted (hit region present) → capture retained.
    state.release_capture_if_unmounted_for_test();
    ensure_eq!(
        *state.capture_target.borrow(),
        Some(a),
        "capture retained while element is still in the hit list"
    );

    // Simulate a frame where the element produced no hit region (unmounted).
    state.hit_test_list.borrow_mut().clear();
    state.release_capture_if_unmounted_for_test();
    ensure_eq!(
        *state.capture_target.borrow(),
        None,
        "capture auto-releases when the captured element is unmounted"
    );
    ensure!(
        !*state.explicit_capture.borrow(),
        "auto-release also clears the explicit flag"
    );
    Ok(())
}

fn check_window_id_matches_in_mouse_and_key_handlers() -> Result<(), String> {
    let state = make_state();
    let expected = state.window_id_for_test();

    // Mouse handler. The mouse-handler trait object is `Send + Sync`, so the
    // capture cell must be too — use `Arc<Mutex<_>>`, not `Rc<Cell<_>>`.
    let from_mouse: Arc<Mutex<Option<slate_platform::WindowId>>> = Arc::new(Mutex::new(None));
    let fm = from_mouse.clone();
    let a = id(1);
    wire_down_handler(
        &state,
        a,
        bounds(0.0, 0.0, 100.0, 100.0),
        Arc::new(move |_ev, cx| *fm.lock().unwrap() = Some(cx.window_id())),
    );
    state.dispatch_mouse_down_for_test((10.0, 10.0), MouseButton::Left, Modifiers::default());
    ensure_eq!(
        *from_mouse.lock().unwrap(),
        Some(expected),
        "mouse handler observes the originating window id"
    );

    // App-level key handler.
    let from_key: Rc<Cell<Option<slate_platform::WindowId>>> = Rc::new(Cell::new(None));
    let fk = from_key.clone();
    state.install_key_handlers_for_test(
        vec![Box::new(move |_e: &KeyEvent, cx: &mut EventCtx| {
            fk.set(Some(cx.window_id()))
        })],
        vec![],
        vec![],
    );
    state.dispatch_key_down_for_test(
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
        false,
    );
    ensure_eq!(
        from_key.get(),
        Some(expected),
        "key handler observes the originating window id"
    );
    Ok(())
}

fn main() {
    let cases: &[Case] = &[
        (
            "set_capture_overrides_and_marks_explicit",
            check_set_capture_overrides_and_marks_explicit,
        ),
        ("release_capture_clears_target", check_release_capture_clears_target),
        (
            "explicit_capture_survives_mouse_up",
            check_explicit_capture_survives_mouse_up,
        ),
        (
            "unmounted_captured_element_auto_releases",
            check_unmounted_captured_element_auto_releases,
        ),
        (
            "window_id_matches_in_mouse_and_key_handlers",
            check_window_id_matches_in_mouse_and_key_handlers,
        ),
    ];

    let mut failed = 0;
    for (name, f) in cases {
        match f() {
            Ok(()) => println!("ok   - {name}"),
            Err(e) => {
                eprintln!("FAIL - {name}: {e}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        eprintln!("\n{failed} case(s) failed");
        std::process::exit(1);
    }
    println!("\nall {} case(s) passed", cases.len());
}
