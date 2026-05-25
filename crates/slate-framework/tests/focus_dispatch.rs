//! Per-element focused-chain keyboard dispatch tests.
//!
//! Windows-only for the same reason as `keyboard_dispatch.rs`: `DefaultPlatform`
//! cannot construct off the main thread on macOS, and `cargo test` runs each
//! test on a worker. The dispatch logic itself is platform-agnostic — verifying
//! on Windows verifies the framework path for both OSes. macOS coverage rolls
//! up through manual smoke via `reactive-counter`.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slate_framework::app_state::{AppSignal, AppState};
use slate_framework::app_state::window_state::WindowState;
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::event::KeyHandlers;
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::focus::FocusableEntry;
use slate_framework::hit_test::HitRegion;
use slate_framework::types::{Bounds, ElementId};
use slate_framework::view::{IntoAny, View};
use slate_framework::{
    EventCtx, Key, KeyCode, KeyEvent, Modifiers, MouseButton, NamedKey, TextInputEvent,
};
use slate_platform::{DefaultPlatform, Platform, Window, WindowId, WindowOptions, wake_run_loop};

#[allow(dead_code)]
struct NoopView;

impl View for NoopView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new().into_any()
    }
}

fn make_state() -> (Rc<AppState>, WindowId) {
    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-focus-test".into(),
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
    let state = Rc::new(AppState::new(executor, redraw_requester.clone(), runtime.clone()));
    let window_id = window.id();
    {
        let win_state = WindowState::new(window, runtime);
        state.windows.borrow_mut().insert(window_id, win_state);
    }
    state.register_redraw_requester_for_test(window_id, redraw_requester);
    (state, window_id)
}

fn id(n: u64) -> ElementId {
    ElementId::from_raw(n)
}

fn entry(n: u64) -> FocusableEntry {
    FocusableEntry {
        id: id(n),
        tab_index: 0,
        focus_ring: true,
    }
}

#[test]
fn focused_element_receives_key_down() {
    let (state, win) = make_state();
    let leaf = id(1);
    state.register_focusable_for_test(win, entry(1));
    state.set_focus_for_test(win, leaf);

    let fired = Arc::new(Mutex::new(0u32));
    let f = fired.clone();
    state.install_element_key_handlers_for_test(
        win,
        leaf,
        KeyHandlers {
            on_key_down: Some(Arc::new(move |_e, _cx| *f.lock().unwrap() += 1)),
            on_key_up: None,
            on_text_input: None,
        },
    );

    let signal = state.dispatch_key_down_for_test(
        win,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
        false,
    );

    assert_eq!(*fired.lock().unwrap(), 1);
    assert!(matches!(signal, AppSignal::RequestRedraw { .. }));
}

#[test]
fn key_down_bubbles_leaf_to_root_to_app_level() {
    let (state, win) = make_state();
    let leaf = id(1);
    let parent = id(2);
    state.register_focusable_for_test(win, entry(1));
    state.set_parent_for_test(win, leaf, parent);
    state.set_focus_for_test(win, leaf);

    let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));
    let o1 = order.clone();
    let o2 = order.clone();
    let o3 = order.clone();

    state.install_element_key_handlers_for_test(
        win,
        leaf,
        KeyHandlers {
            on_key_down: Some(Arc::new(move |_e, _cx| o1.lock().unwrap().push("leaf"))),
            ..Default::default()
        },
    );
    state.install_element_key_handlers_for_test(
        win,
        parent,
        KeyHandlers {
            on_key_down: Some(Arc::new(move |_e, _cx| o2.lock().unwrap().push("parent"))),
            ..Default::default()
        },
    );
    state.install_key_handlers_for_test(
        vec![Box::new(move |_e: &KeyEvent, _cx: &mut EventCtx| {
            o3.lock().unwrap().push("app")
        })],
        vec![],
        vec![],
    );

    state.dispatch_key_down_for_test(
        win,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
        false,
    );

    assert_eq!(&*order.lock().unwrap(), &vec!["leaf", "parent", "app"]);
}

#[test]
fn stop_propagation_halts_bubble() {
    let (state, win) = make_state();
    let leaf = id(1);
    let parent = id(2);
    state.register_focusable_for_test(win, entry(1));
    state.set_parent_for_test(win, leaf, parent);
    state.set_focus_for_test(win, leaf);

    let parent_fired = Arc::new(Mutex::new(false));
    let app_fired = Arc::new(Mutex::new(false));
    let pf = parent_fired.clone();
    let af = app_fired.clone();

    state.install_element_key_handlers_for_test(
        win,
        leaf,
        KeyHandlers {
            on_key_down: Some(Arc::new(|_e, cx| cx.stop_propagation())),
            ..Default::default()
        },
    );
    state.install_element_key_handlers_for_test(
        win,
        parent,
        KeyHandlers {
            on_key_down: Some(Arc::new(move |_e, _cx| *pf.lock().unwrap() = true)),
            ..Default::default()
        },
    );
    state.install_key_handlers_for_test(
        vec![Box::new(move |_e: &KeyEvent, _cx: &mut EventCtx| {
            *af.lock().unwrap() = true
        })],
        vec![],
        vec![],
    );

    state.dispatch_key_down_for_test(
        win,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
        false,
    );

    assert!(
        !*parent_fired.lock().unwrap(),
        "parent must not fire after stop_propagation"
    );
    assert!(
        !*app_fired.lock().unwrap(),
        "app-level must not fire after stop_propagation"
    );
}

#[test]
fn text_input_bubbles_focused_chain() {
    let (state, win) = make_state();
    let leaf = id(1);
    state.register_focusable_for_test(win, entry(1));
    state.set_focus_for_test(win, leaf);

    let captured = Arc::new(Mutex::new(String::new()));
    let c = captured.clone();
    state.install_element_key_handlers_for_test(
        win,
        leaf,
        KeyHandlers {
            on_text_input: Some(Arc::new(move |e: &TextInputEvent, _cx| {
                *c.lock().unwrap() = e.text.clone()
            })),
            ..Default::default()
        },
    );

    state.dispatch_text_input_for_test(win, "hi".into());

    assert_eq!(&*captured.lock().unwrap(), "hi");
}

#[test]
fn request_focus_applied_after_handler_chain() {
    let (state, win) = make_state();
    let a = id(1);
    let b = id(2);
    state.register_focusable_for_test(win, entry(1));
    state.register_focusable_for_test(win, entry(2));
    state.set_focus_for_test(win, a);

    state.install_element_key_handlers_for_test(
        win,
        a,
        KeyHandlers {
            on_key_down: Some(Arc::new(move |_e, cx| {
                cx.request_focus(b);
            })),
            ..Default::default()
        },
    );

    state.dispatch_key_down_for_test(
        win,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
        false,
    );

    assert_eq!(state.focused_for_test(win), Some(b));
}

#[test]
fn tab_key_default_advances_focus() {
    let (state, win) = make_state();
    state.register_focusable_for_test(win, entry(1));
    state.register_focusable_for_test(win, entry(2));
    state.register_focusable_for_test(win, entry(3));
    state.set_focus_for_test(win, id(1));

    state.dispatch_key_down_for_test(
        win,
        KeyCode::Tab,
        Key::Named(NamedKey::Tab),
        Modifiers::default(),
        false,
    );

    assert_eq!(state.focused_for_test(win), Some(id(2)));
}

#[test]
fn tab_key_with_stop_propagation_suppresses_default_shift() {
    let (state, win) = make_state();
    let a = id(1);
    state.register_focusable_for_test(win, entry(1));
    state.register_focusable_for_test(win, entry(2));
    state.set_focus_for_test(win, a);

    state.install_element_key_handlers_for_test(
        win,
        a,
        KeyHandlers {
            on_key_down: Some(Arc::new(|_e, cx| cx.stop_propagation())),
            ..Default::default()
        },
    );

    state.dispatch_key_down_for_test(
        win,
        KeyCode::Tab,
        Key::Named(NamedKey::Tab),
        Modifiers::default(),
        false,
    );

    assert_eq!(
        state.focused_for_test(win),
        Some(a),
        "Tab default must be suppressed"
    );
}

// ---------------------------------------------------------------------------
// Mouse-down focus semantics (clear focus on background-click)
// ---------------------------------------------------------------------------

/// Helper: install one hit region covering the given rect for `id`. Resets
/// the list first so consecutive setups don't accumulate stale regions.
fn install_hit_region(state: &AppState, win: WindowId, id: ElementId, bounds: Bounds) {
    state.clear_hit_test_list_for_test(win);
    state.push_hit_region_for_test(win, HitRegion::new(id, bounds, 0));
}

#[test]
fn mouse_down_on_non_focusable_clears_focus() {
    // Pre-condition: `a` is focusable + currently focused. Hit region for
    // `b` is staged where `b` is registered with no entry in focus registry.
    // Clicking on `b` (non-focusable, no focusable ancestor) must blur `a`.
    let (state, win) = make_state();
    let a = id(1);
    let b = id(2);
    state.register_focusable_for_test(win, entry(1));
    state.set_focus_for_test(win, a);
    install_hit_region(&state, win, b, Bounds::from_origin_size(0.0, 0.0, 100.0, 100.0));

    state.dispatch_mouse_down_for_test(
        win,
        (10.0, 10.0),
        MouseButton::Left,
        Modifiers::default(),
    );

    assert_eq!(
        state.focused_for_test(win),
        None,
        "background-click on non-focusable target must clear focus"
    );
}

#[test]
fn mouse_down_hit_test_miss_clears_focus() {
    // Pre-condition: `a` is focusable + focused. Hit list contains no regions
    // covering the click position — the hit-test returns None. Focus must
    // clear (native-widget semantics for click on empty space).
    let (state, win) = make_state();
    let a = id(1);
    state.register_focusable_for_test(win, entry(1));
    state.set_focus_for_test(win, a);
    // Empty hit list.
    state.clear_hit_test_list_for_test(win);

    state.dispatch_mouse_down_for_test(
        win,
        (50.0, 50.0),
        MouseButton::Left,
        Modifiers::default(),
    );

    assert_eq!(
        state.focused_for_test(win),
        None,
        "hit-test miss must clear focus"
    );
}

#[test]
fn mouse_down_on_focusable_preserves_focus_move() {
    // Regression guard: clicking on a focusable element still moves focus
    // there (the D6 reversal must not break the click-to-focus path).
    let (state, win) = make_state();
    let a = id(1);
    let b = id(2);
    state.register_focusable_for_test(win, entry(1));
    state.register_focusable_for_test(win, entry(2));
    state.set_focus_for_test(win, a);
    install_hit_region(&state, win, b, Bounds::from_origin_size(0.0, 0.0, 100.0, 100.0));

    state.dispatch_mouse_down_for_test(
        win,
        (25.0, 25.0),
        MouseButton::Left,
        Modifiers::default(),
    );

    assert_eq!(
        state.focused_for_test(win),
        Some(b),
        "click on a focusable element must move focus to it"
    );
}
