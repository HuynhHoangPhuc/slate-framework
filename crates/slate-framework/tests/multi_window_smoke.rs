//! Integration smoke tests for the per-window state partition.
//!
//! Exercises two independently-registered windows sharing one `AppState`.
//! Verifies that:
//!  - per-window state (capture, focus, mouse position, hover, button-state,
//!    recovery-state) is isolated between windows
//!  - the process-wide executor, runtime, and text system are the same
//!    instances for both windows (shared-state partition)
//!  - `handle_window_destroyed` returns no quit signal while one window
//!    survives, then the platform-appropriate signal once the last window closes
//!  - the reactive wake bridge posts exactly one wake per signal change
//!    (single PostMessage on Win32); fan-out to every window happens later
//!    inside `handle_wake`, which invalidates each live window directly.
//!    Posting one wake per registered requester recurses through the wake
//!    handler and floods the OS message queue with WM_APP_WAKE, starving
//!    WM_PAINT — the multi-window hang reproduced when both windows shared
//!    a `RedrawRequester` wrapping `wake_run_loop`.
//!
//! Runs cross-platform under `harness = false` so its `fn main` executes on
//! the process main thread. Required on macOS for `MainThreadMarker`.
//! Gated on `required-features = ["test-hooks"]`.

#![cfg(all(
    any(target_os = "macos", target_os = "windows"),
    feature = "test-hooks"
))]

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slate_framework::app_state::{AppSignal, AppState};
use slate_framework::app_state::window_state::WindowState;
use slate_framework::event::{KeyEvent, KeyHandlers, MouseHandlers, MouseEvent, EventCtx};
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::hit_test::HitRegion;
use slate_framework::types::{Bounds, ElementId, Point, Size};
use slate_framework::{Key, KeyCode, Modifiers, MouseButton};
use slate_platform::{DefaultPlatform, Platform, Window, WindowId, WindowOptions, wake_run_loop};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

type Case = (&'static str, fn() -> Result<(), String>);

/// Element-bound mouse handler — must be `Send + Sync` per the framework contract.
type ArcMouse = Arc<dyn Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static>;

/// Element-bound key handler — must be `Send + Sync` per the framework contract.
type ArcKey = Arc<dyn Fn(&KeyEvent, &mut EventCtx) + Send + Sync + 'static>;

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

fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds {
    Bounds {
        origin: Point::new(x, y),
        size: Size::new(w, h),
    }
}

fn id(n: u64) -> ElementId {
    ElementId::from_raw(n)
}

/// Construct an `AppState` with TWO registered test windows.
///
/// Returns the shared state plus the two distinct `WindowId`s. Both windows
/// share the same `AppState` (and therefore the same executor, runtime, and
/// text system), while each owns its own `WindowState`.
fn make_two_window_state() -> (Rc<AppState>, WindowId, WindowId) {
    let platform = DefaultPlatform::new();

    let window_a = platform.create_window(WindowOptions {
        title: "slate-smoke-window-a".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32000, -32000)),
    });
    let window_b = platform.create_window(WindowOptions {
        title: "slate-smoke-window-b".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32001, -32001)),
    });

    let redraw_a = RedrawRequester::new(wake_run_loop);
    let redraw_b = RedrawRequester::new(wake_run_loop);
    // Executor only needs one requester for the shared foreground pool.
    let executor = Executor::new(redraw_a.clone());
    let runtime = slate_reactive::Runtime::new();

    let state = Rc::new(AppState::new(executor, redraw_a.clone(), runtime.clone()));

    let id_a = window_a.id();
    let id_b = window_b.id();

    {
        let win_a = WindowState::new(window_a, runtime.clone());
        let win_b = WindowState::new(window_b, runtime);
        let mut windows = state.windows.borrow_mut();
        windows.insert(id_a, win_a);
        windows.insert(id_b, win_b);
    }

    state.register_redraw_requester_for_test(id_a, redraw_a);
    state.register_redraw_requester_for_test(id_b, redraw_b);

    // Keep platform alive until end of test by leaking the binding.
    let _ = platform;

    (state, id_a, id_b)
}

// ---------------------------------------------------------------------------
// Test: per-window capture state is independent
// ---------------------------------------------------------------------------

fn check_capture_state_is_per_window() -> Result<(), String> {
    let (state, win_a, win_b) = make_two_window_state();
    let elem_a = id(1);
    let elem_b = id(2);

    // Install a hit region + capture-setting handler on window A only.
    let on_down: ArcMouse = Arc::new(move |_ev, cx| cx.set_capture(elem_a));
    state.install_element_mouse_handlers_for_test(
        win_a,
        elem_a,
        MouseHandlers {
            on_mouse_down: Some(on_down),
            on_mouse_move: None,
            on_mouse_up: None,
        },
    );
    state.push_hit_region_for_test(win_a, HitRegion::new(elem_a, bounds(0.0, 0.0, 100.0, 100.0), 0));

    // Install a separate hit region on window B — no capture.
    let on_down_b: ArcMouse = Arc::new(move |_ev, _cx| {});
    state.install_element_mouse_handlers_for_test(
        win_b,
        elem_b,
        MouseHandlers {
            on_mouse_down: Some(on_down_b),
            on_mouse_move: None,
            on_mouse_up: None,
        },
    );
    state.push_hit_region_for_test(win_b, HitRegion::new(elem_b, bounds(0.0, 0.0, 100.0, 100.0), 0));

    // Click on window A → capture should be set on A, not B.
    state.dispatch_mouse_down_for_test(win_a, (10.0, 10.0), MouseButton::Left, Modifiers::default());

    ensure_eq!(
        state.capture_target_for_test(win_a),
        Some(elem_a),
        "window A should have capture set"
    );
    ensure_eq!(
        state.capture_target_for_test(win_b),
        None,
        "window B must not inherit window A's capture"
    );

    // Click on window B — B gets its own capture set by auto-capture logic.
    state.dispatch_mouse_down_for_test(win_b, (10.0, 10.0), MouseButton::Left, Modifiers::default());
    ensure_eq!(
        state.capture_target_for_test(win_a),
        Some(elem_a),
        "window A capture must survive the window B click"
    );
    // B auto-captures to the hit element on mouse-down.
    ensure_eq!(
        state.capture_target_for_test(win_b),
        Some(elem_b),
        "window B should have its own capture after mouse-down"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test: per-window focus state is independent
// ---------------------------------------------------------------------------

fn check_focus_state_is_per_window() -> Result<(), String> {
    use slate_framework::focus::FocusableEntry;

    let (state, win_a, win_b) = make_two_window_state();

    // Register focusable elements in each window.
    state.register_focusable_for_test(win_a, FocusableEntry { id: id(10), tab_index: 0, focus_ring: true });
    state.register_focusable_for_test(win_b, FocusableEntry { id: id(20), tab_index: 0, focus_ring: true });

    state.set_focus_for_test(win_a, id(10));
    state.set_focus_for_test(win_b, id(20));

    ensure_eq!(
        state.focused_for_test(win_a),
        Some(id(10)),
        "window A focus must be element 10"
    );
    ensure_eq!(
        state.focused_for_test(win_b),
        Some(id(20)),
        "window B focus must be element 20, independent of A"
    );

    // Changing focus on A must not affect B.
    state.register_focusable_for_test(win_a, FocusableEntry { id: id(11), tab_index: 1, focus_ring: true });
    state.set_focus_for_test(win_a, id(11));

    ensure_eq!(
        state.focused_for_test(win_a),
        Some(id(11)),
        "window A focus should now be element 11"
    );
    ensure_eq!(
        state.focused_for_test(win_b),
        Some(id(20)),
        "window B focus must remain on element 20"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test: last-window-close semantics
// ---------------------------------------------------------------------------

fn check_last_window_destroy_semantics() -> Result<(), String> {
    let (state, win_a, win_b) = make_two_window_state();

    // Destroying the first window must NOT signal quit (one window remains).
    let signal_a = state.handle_window_destroyed(win_a);
    ensure!(
        !matches!(signal_a, AppSignal::RequestQuit),
        "destroying window A (one window left) should NOT return RequestQuit"
    );

    // After first destroy, the second window must still be alive.
    ensure!(
        state.windows.borrow().contains_key(&win_b),
        "window B must still exist after window A is destroyed"
    );

    // Destroying the last window should produce the platform-appropriate signal.
    let signal_b = state.handle_window_destroyed(win_b);

    #[cfg(target_os = "windows")]
    ensure!(
        matches!(signal_b, AppSignal::RequestQuit),
        "destroying the last window on Windows must return RequestQuit (platform default)"
    );
    #[cfg(target_os = "macos")]
    ensure!(
        matches!(signal_b, AppSignal::None),
        "destroying the last window on macOS must return None (AppKit stays alive)"
    );

    // Windows map must be empty.
    ensure!(
        state.windows.borrow().is_empty(),
        "windows map must be empty after both windows destroyed"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test: reactive wake-all bridge notifies all registered requesters
// ---------------------------------------------------------------------------

fn check_redraw_bridge_wakes_once_per_signal_change() -> Result<(), String> {
    let platform = DefaultPlatform::new();

    let window_a = platform.create_window(WindowOptions {
        title: "slate-redraw-bridge-test-a".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32000, -32000)),
    });
    let window_b = platform.create_window(WindowOptions {
        title: "slate-redraw-bridge-test-b".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32001, -32001)),
    });

    // Counting requesters: track how many times each was called.
    let count_a = Arc::new(Mutex::new(0u32));
    let count_b = Arc::new(Mutex::new(0u32));

    let ca = count_a.clone();
    let cb = count_b.clone();

    let req_a = RedrawRequester::new(move || { *ca.lock().unwrap() += 1; });
    let req_b = RedrawRequester::new(move || { *cb.lock().unwrap() += 1; });

    let executor = Executor::new(req_a.clone());
    let runtime = slate_reactive::Runtime::new();

    let state = Rc::new(AppState::new(executor, req_a.clone(), runtime.clone()));

    let id_a = window_a.id();
    let id_b = window_b.id();

    {
        let win_a = WindowState::new(window_a, runtime.clone());
        let win_b = WindowState::new(window_b, runtime.clone());
        let mut windows = state.windows.borrow_mut();
        windows.insert(id_a, win_a);
        windows.insert(id_b, win_b);
    }

    state.register_redraw_requester_for_test(id_a, req_a.clone());
    state.register_redraw_requester_for_test(id_b, req_b.clone());

    let _ = platform;

    // Create a signal and subscribe via an effect to wire the runtime's
    // redraw bridge. Signal::set() triggers the bridge's closure, which
    // should call request() on BOTH registered requesters.
    let sig = slate_reactive::Signal::new(runtime.clone(), 0u32);

    // Capture counts before the signal fires.
    let before_a = *count_a.lock().unwrap();
    let before_b = *count_b.lock().unwrap();

    // Mutating the signal fires the redraw bridge callback registered in
    // AppState::new (the closure captures Arc<Mutex<Vec<RedrawRequester>>>).
    sig.set(1);

    let after_a = *count_a.lock().unwrap();
    let after_b = *count_b.lock().unwrap();

    // New contract (Win32 WM_APP_WAKE flood fix): the install_redraw bridge
    // wakes exactly ONCE per signal change. Fan-out to every window happens
    // inside `handle_wake`, not at the install_redraw layer. Fanning out
    // here previously turned N>=2 windows into an exponential WM_APP_WAKE
    // flood that starved WM_PAINT.
    //
    // Window A's requester (first in the Vec) is the one woken; window B's
    // requester is intentionally NOT called by the bridge.
    ensure!(
        after_a == before_a + 1,
        "wake bridge must fire exactly once for window A (before={}, after={})",
        before_a,
        after_a,
    );
    ensure!(
        after_b == before_b,
        "wake bridge must NOT iterate per-window requesters (would re-introduce WM_APP_WAKE flood); window B count must stay at {} (after={})",
        before_b,
        after_b,
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test: dispatching to one window does not affect the other window's state
// ---------------------------------------------------------------------------

fn check_mouse_move_dispatch_is_per_window() -> Result<(), String> {
    let (state, win_a, win_b) = make_two_window_state();

    // Move mouse over window A.
    state.dispatch_mouse_move_for_test(win_a, (42.0, 17.0));

    // Window B's last_mouse_pos must not be affected.
    let last_pos_b = state.windows.borrow().get(&win_b).map(|w| *w.last_mouse_pos.borrow());
    ensure_eq!(
        last_pos_b,
        Some(None),
        "window B last_mouse_pos must remain None after window A received a move"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test: key dispatch to window A does not route to window B's focused element
// ---------------------------------------------------------------------------

fn check_key_dispatch_routes_to_correct_window() -> Result<(), String> {
    use slate_framework::focus::FocusableEntry;

    let (state, win_a, win_b) = make_two_window_state();

    // Arc<Mutex<bool>> required: element key handlers must be Send + Sync.
    let fired_a: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let fired_b: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));

    // Register focusable + key handler on window A.
    state.register_focusable_for_test(win_a, FocusableEntry { id: id(1), tab_index: 0, focus_ring: false });
    state.set_focus_for_test(win_a, id(1));
    {
        let fa = fired_a.clone();
        let h: ArcKey = Arc::new(move |_ev, _cx| *fa.lock().unwrap() = true);
        state.install_element_key_handlers_for_test(win_a, id(1), KeyHandlers { on_key_down: Some(h), on_key_up: None, on_text_input: None });
    }

    // Register focusable + key handler on window B.
    state.register_focusable_for_test(win_b, FocusableEntry { id: id(2), tab_index: 0, focus_ring: false });
    state.set_focus_for_test(win_b, id(2));
    {
        let fb = fired_b.clone();
        let h: ArcKey = Arc::new(move |_ev, _cx| *fb.lock().unwrap() = true);
        state.install_element_key_handlers_for_test(win_b, id(2), KeyHandlers { on_key_down: Some(h), on_key_up: None, on_text_input: None });
    }

    // Dispatch key to window A.
    state.dispatch_key_down_for_test(
        win_a,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
        false,
    );

    ensure!(
        *fired_a.lock().unwrap(),
        "window A's element key handler must fire when key dispatched to window A"
    );
    ensure!(
        !*fired_b.lock().unwrap(),
        "window B's element key handler must NOT fire when key dispatched to window A"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Main harness
// ---------------------------------------------------------------------------

fn main() {
    let cases: &[Case] = &[
        ("capture_state_is_per_window", check_capture_state_is_per_window),
        ("focus_state_is_per_window", check_focus_state_is_per_window),
        ("last_window_destroy_semantics", check_last_window_destroy_semantics),
        ("redraw_bridge_wakes_once_per_signal_change", check_redraw_bridge_wakes_once_per_signal_change),
        ("mouse_move_dispatch_is_per_window", check_mouse_move_dispatch_is_per_window),
        ("key_dispatch_routes_to_correct_window", check_key_dispatch_routes_to_correct_window),
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
