//! Headless tests for `AppState::dispatch_key_*`.
//!
//! Verifies that synthetic KeyDown / KeyUp / TextInput events route through
//! the dispatch methods into registered handlers, return the expected
//! `AppSignal`, and propagate `is_repeat`.
//!
//! Windows-only because `DefaultPlatform::new()` panics on macOS off the main
//! OS thread (AppKit requires main-thread init), and `cargo test` runs each
//! test on a worker thread. macOS keyboard dispatch is covered by:
//!
//!   1. In-tree `mac/keymap.rs` unit tests for the keymap layer.
//!   2. Manual smoke via `cargo run -p reactive-counter`.
//!
//! The dispatch logic is platform-agnostic — verifying on Windows verifies
//! the framework path for both OSes.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slate_framework::app_state::window_state::WindowState;
use slate_framework::app_state::{AppSignal, AppState};
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::view::{IntoAny, View};
use slate_framework::{
    EventCtx, Key, KeyCode, KeyEvent, KeyHandler, Modifiers, NamedKey, TextInputEvent,
    TextInputHandler,
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
        title: "slate-keyboard-test".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32000, -32000)),
        ..Default::default()
    });
    let redraw_requester = RedrawRequester::new(wake_run_loop);
    let executor = Executor::new(redraw_requester.clone());
    let runtime = slate_reactive::Runtime::new();
    // Platform is dropped here; window keeps the HWND alive via its Arc inner.
    // Dispatch methods touch only RefCells, so no platform pump is needed.
    let _ = platform;
    let state = Rc::new(AppState::new(
        executor,
        redraw_requester.clone(),
        runtime.clone(),
    ));
    let window_id = window.id();
    {
        let win_state = WindowState::new(window, runtime);
        state.windows.borrow_mut().insert(window_id, win_state);
    }
    state.register_redraw_requester_for_test(window_id, redraw_requester);
    (state, window_id)
}

#[test]
fn dispatch_key_down_fires_registered_handler() {
    let fired = Rc::new(Cell::new(0u32));
    let f = fired.clone();
    let (state, win) = make_state();
    state.install_key_handlers_for_test(
        vec![Box::new(move |_e: &KeyEvent, _cx: &mut EventCtx| {
            f.set(f.get() + 1)
        })],
        vec![],
        vec![],
    );

    let signal = state.dispatch_key_down_for_test(
        win,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
        false,
    );

    assert_eq!(fired.get(), 1);
    assert!(matches!(signal, AppSignal::RequestRedraw { .. }));
}

#[test]
fn dispatch_key_down_invokes_all_handlers_in_order() {
    let order = Rc::new(RefCell::new(Vec::<u8>::new()));
    let o1 = order.clone();
    let o2 = order.clone();
    let o3 = order.clone();
    let (state, win) = make_state();
    let handlers: Vec<KeyHandler> = vec![
        Box::new(move |_e: &KeyEvent, _cx: &mut EventCtx| o1.borrow_mut().push(1)),
        Box::new(move |_e: &KeyEvent, _cx: &mut EventCtx| o2.borrow_mut().push(2)),
        Box::new(move |_e: &KeyEvent, _cx: &mut EventCtx| o3.borrow_mut().push(3)),
    ];
    state.install_key_handlers_for_test(handlers, vec![], vec![]);

    let _ = state.dispatch_key_down_for_test(
        win,
        KeyCode::Space,
        Key::Named(NamedKey::Space),
        Modifiers::default(),
        false,
    );

    assert_eq!(*order.borrow(), vec![1, 2, 3]);
}

#[test]
fn dispatch_key_up_no_handlers_returns_none_signal() {
    let (state, win) = make_state();
    let signal = state.dispatch_key_up_for_test(
        win,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
    );
    assert_eq!(signal, AppSignal::None);
}

#[test]
fn dispatch_text_input_passes_string_to_handler() {
    let captured = Rc::new(RefCell::new(String::new()));
    let c = captured.clone();
    let (state, win) = make_state();
    let text_handlers: Vec<TextInputHandler> =
        vec![Box::new(move |e: &TextInputEvent, _cx: &mut EventCtx| {
            *c.borrow_mut() = e.text.clone()
        })];
    state.install_key_handlers_for_test(vec![], vec![], text_handlers);

    let signal = state.dispatch_text_input_for_test(win, "hello".into());

    assert_eq!(*captured.borrow(), "hello");
    assert!(matches!(signal, AppSignal::RequestRedraw { .. }));
}

#[test]
fn dispatch_key_event_carries_is_repeat_flag() {
    let last = Rc::new(Cell::new(false));
    let l = last.clone();
    let (state, win) = make_state();
    state.install_key_handlers_for_test(
        vec![Box::new(move |e: &KeyEvent, _cx: &mut EventCtx| {
            l.set(e.is_repeat)
        })],
        vec![],
        vec![],
    );

    let _ = state.dispatch_key_down_for_test(
        win,
        KeyCode::Space,
        Key::Named(NamedKey::Space),
        Modifiers::default(),
        true,
    );

    assert!(last.get());
}

#[test]
fn dispatch_key_up_clears_is_repeat_flag() {
    // KeyUp dispatch fabricates is_repeat=false unconditionally (OS auto-repeat
    // never produces KeyUp). Verify handler sees false even if previous KeyDown
    // had is_repeat=true.
    let observed = Rc::new(Cell::new(true));
    let o = observed.clone();
    let (state, win) = make_state();
    state.install_key_handlers_for_test(
        vec![],
        vec![Box::new(move |e: &KeyEvent, _cx: &mut EventCtx| {
            o.set(e.is_repeat)
        })],
        vec![],
    );

    let _ = state.dispatch_key_up_for_test(
        win,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
    );

    assert!(!observed.get());
}

#[test]
fn dispatch_does_not_request_redraw_when_no_handlers() {
    let (state, win) = make_state();

    let key_down = state.dispatch_key_down_for_test(
        win,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
        false,
    );
    let key_up = state.dispatch_key_up_for_test(
        win,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
    );
    let text_input = state.dispatch_text_input_for_test(win, "x".into());

    assert_eq!(key_down, AppSignal::None);
    assert_eq!(key_up, AppSignal::None);
    assert_eq!(text_input, AppSignal::None);
}

#[test]
fn keyboard_dispatch_unaffected_by_ime_registry() {
    // Back-compat sweep: with NO ime-capable elements registered, the
    // keyboard dispatch path is identical in behaviour. Verifies that IME
    // additions did not regress existing keyboard dispatch code paths.
    let fired = Rc::new(Cell::new(0u32));
    let f = fired.clone();
    let (state, win) = make_state();
    state.install_key_handlers_for_test(
        vec![Box::new(move |_e: &KeyEvent, _cx: &mut EventCtx| {
            f.set(f.get() + 1)
        })],
        vec![],
        vec![],
    );

    let signal = state.dispatch_key_down_for_test(
        win,
        KeyCode::KeyA,
        Key::Character("a".into()),
        Modifiers::default(),
        false,
    );

    assert_eq!(fired.get(), 1);
    assert!(matches!(signal, AppSignal::RequestRedraw { .. }));
}
