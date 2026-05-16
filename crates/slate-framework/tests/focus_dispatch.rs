//! Per-element focused-chain keyboard dispatch tests (Phase 9b).
//!
//! Windows-only for the same reason as `keyboard_dispatch.rs`: `DefaultPlatform`
//! cannot construct off the main thread on macOS, and `cargo test` runs each
//! test on a worker. The dispatch logic itself is platform-agnostic — verifying
//! on Windows verifies the framework path for both OSes. macOS coverage rolls
//! up through manual smoke via `reactive-counter`.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use slate_framework::app_state::{AppSignal, AppState};
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::focus::FocusableEntry;
use slate_framework::types::ElementId;
use slate_framework::view::{IntoAny, View};
use slate_framework::{
    EventCtx, Key, KeyCode, KeyEvent, Modifiers, NamedKey, TextInputEvent,
};
use slate_framework::event::KeyHandlers;
use slate_platform::{DefaultPlatform, Platform, WindowOptions, wake_run_loop};

struct NoopView;

impl View for NoopView {
    fn render(&mut self) -> AnyElement {
        Div::new().into_any()
    }
}

fn make_state() -> Rc<AppState<NoopView>> {
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
    Rc::new(AppState::new(window, executor, redraw_requester, runtime))
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
    let state = make_state();
    let leaf = id(1);
    state.register_focusable_for_test(entry(1));
    state.set_focus_for_test(leaf);

    let fired = Rc::new(Cell::new(0u32));
    let f = fired.clone();
    state.install_element_key_handlers_for_test(
        leaf,
        KeyHandlers {
            on_key_down: Some(Arc::new(move |_e, _cx| f.set(f.get() + 1))),
            on_key_up: None,
            on_text_input: None,
        },
    );

    let signal = state.dispatch_key_down_for_test(
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
        false,
    );

    assert_eq!(fired.get(), 1);
    assert_eq!(signal, AppSignal::RequestRedraw);
}

#[test]
fn key_down_bubbles_leaf_to_root_to_app_level() {
    let state = make_state();
    let leaf = id(1);
    let parent = id(2);
    state.register_focusable_for_test(entry(1));
    state.set_parent_for_test(leaf, parent);
    state.set_focus_for_test(leaf);

    let order = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    let o1 = order.clone();
    let o2 = order.clone();
    let o3 = order.clone();

    state.install_element_key_handlers_for_test(
        leaf,
        KeyHandlers {
            on_key_down: Some(Arc::new(move |_e, _cx| o1.borrow_mut().push("leaf"))),
            ..Default::default()
        },
    );
    state.install_element_key_handlers_for_test(
        parent,
        KeyHandlers {
            on_key_down: Some(Arc::new(move |_e, _cx| o2.borrow_mut().push("parent"))),
            ..Default::default()
        },
    );
    state.install_key_handlers_for_test(
        vec![Box::new(move |_e: &KeyEvent, _cx: &mut EventCtx| {
            o3.borrow_mut().push("app")
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

    assert_eq!(&*order.borrow(), &vec!["leaf", "parent", "app"]);
}

#[test]
fn stop_propagation_halts_bubble() {
    let state = make_state();
    let leaf = id(1);
    let parent = id(2);
    state.register_focusable_for_test(entry(1));
    state.set_parent_for_test(leaf, parent);
    state.set_focus_for_test(leaf);

    let parent_fired = Rc::new(Cell::new(false));
    let app_fired = Rc::new(Cell::new(false));
    let pf = parent_fired.clone();
    let af = app_fired.clone();

    state.install_element_key_handlers_for_test(
        leaf,
        KeyHandlers {
            on_key_down: Some(Arc::new(|_e, cx| cx.stop_propagation())),
            ..Default::default()
        },
    );
    state.install_element_key_handlers_for_test(
        parent,
        KeyHandlers {
            on_key_down: Some(Arc::new(move |_e, _cx| pf.set(true))),
            ..Default::default()
        },
    );
    state.install_key_handlers_for_test(
        vec![Box::new(move |_e: &KeyEvent, _cx: &mut EventCtx| {
            af.set(true)
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

    assert!(!parent_fired.get(), "parent must not fire after stop_propagation");
    assert!(!app_fired.get(), "app-level must not fire after stop_propagation");
}

#[test]
fn text_input_bubbles_focused_chain() {
    let state = make_state();
    let leaf = id(1);
    state.register_focusable_for_test(entry(1));
    state.set_focus_for_test(leaf);

    let captured = Rc::new(RefCell::new(String::new()));
    let c = captured.clone();
    state.install_element_key_handlers_for_test(
        leaf,
        KeyHandlers {
            on_text_input: Some(Arc::new(move |e: &TextInputEvent, _cx| {
                *c.borrow_mut() = e.text.clone()
            })),
            ..Default::default()
        },
    );

    state.dispatch_text_input_for_test("hi".into());

    assert_eq!(&*captured.borrow(), "hi");
}

#[test]
fn request_focus_applied_after_handler_chain() {
    let state = make_state();
    let a = id(1);
    let b = id(2);
    state.register_focusable_for_test(entry(1));
    state.register_focusable_for_test(entry(2));
    state.set_focus_for_test(a);

    state.install_element_key_handlers_for_test(
        a,
        KeyHandlers {
            on_key_down: Some(Arc::new(move |_e, cx| {
                cx.request_focus(b);
            })),
            ..Default::default()
        },
    );

    state.dispatch_key_down_for_test(
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
        false,
    );

    assert_eq!(state.focused_for_test(), Some(b));
}

#[test]
fn tab_key_default_advances_focus() {
    let state = make_state();
    state.register_focusable_for_test(entry(1));
    state.register_focusable_for_test(entry(2));
    state.register_focusable_for_test(entry(3));
    state.set_focus_for_test(id(1));

    state.dispatch_key_down_for_test(
        KeyCode::Tab,
        Key::Named(NamedKey::Tab),
        Modifiers::default(),
        false,
    );

    assert_eq!(state.focused_for_test(), Some(id(2)));
}

#[test]
fn tab_key_with_stop_propagation_suppresses_default_shift() {
    let state = make_state();
    let a = id(1);
    state.register_focusable_for_test(entry(1));
    state.register_focusable_for_test(entry(2));
    state.set_focus_for_test(a);

    state.install_element_key_handlers_for_test(
        a,
        KeyHandlers {
            on_key_down: Some(Arc::new(|_e, cx| cx.stop_propagation())),
            ..Default::default()
        },
    );

    state.dispatch_key_down_for_test(
        KeyCode::Tab,
        Key::Named(NamedKey::Tab),
        Modifiers::default(),
        false,
    );

    assert_eq!(state.focused_for_test(), Some(a), "Tab default must be suppressed");
}
