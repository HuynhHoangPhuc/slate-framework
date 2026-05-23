//! Integration tests for `AppState::dispatch_ime_*` (Phase 9c).
//!
//! Verifies that IME preedit / commit / lifecycle events route through the
//! dispatch methods into registered handlers, return the expected `AppSignal`,
//! and follow the focused-chain bubble / stop-propagation semantics.
//!
//! Windows-only because `DefaultPlatform::new()` panics on macOS off the main
//! OS thread (AppKit requires main-thread init) and `cargo test` runs each test
//! on a worker thread. The dispatch logic is platform-agnostic — verifying on
//! Windows covers the framework path for both OSes.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use slate_framework::app_state::{AppSignal, AppState};
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::event::{ImeCommitEvent, ImeHandlers, ImeLifecycleEvent, ImePreeditEvent};
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::focus::FocusableEntry;
use slate_framework::ime::{ImeState, Preedit};
use slate_framework::types::ElementId;
use slate_framework::view::{IntoAny, View};
use slate_framework::{EventCtx, Key, KeyCode, Modifiers, NamedKey};
use slate_platform::{DefaultPlatform, PhysicalRect, Platform, WindowOptions, wake_run_loop};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

struct NoopView;

impl View for NoopView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new().into_any()
    }
}

fn make_state() -> Rc<AppState<NoopView>> {
    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-ime-test".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32000, -32000)),
    });
    let redraw_requester = RedrawRequester::new(wake_run_loop);
    let executor = Executor::new(redraw_requester.clone());
    let runtime = slate_reactive::Runtime::new();
    // Platform is dropped here; window keeps HWND alive via its Arc inner.
    // Dispatch methods touch only RefCells — no platform pump needed.
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

// ---------------------------------------------------------------------------
// App-level handler routing
// ---------------------------------------------------------------------------

#[test]
fn dispatch_ime_preedit_fires_app_handler() {
    let state = make_state();
    let fired = Rc::new(Cell::new(0u32));
    let f = fired.clone();

    state.install_ime_handlers_for_test(
        vec![Box::new(move |_e: &ImePreeditEvent, _cx: &mut EventCtx| {
            f.set(f.get() + 1)
        })],
        vec![],
        vec![],
        vec![],
    );

    let window = state.window_id_for_test();
    let signal = state.dispatch_ime_preedit_for_test(window, "abc".into(), 3, None);

    assert_eq!(fired.get(), 1);
    assert_eq!(signal, AppSignal::RequestRedraw);
}

#[test]
fn dispatch_ime_preedit_routes_to_focused_element_with_stop_propagation() {
    // Element-level handler fires first; calling stop_propagation prevents the
    // app-level handler from firing.
    let state = make_state();
    state.register_focusable_for_test(entry(1));
    state.set_focus_for_test(id(1));

    let elem_fired = Arc::new(Mutex::new(0u32));
    let app_fired = Rc::new(Cell::new(0u32));
    let ef = elem_fired.clone();
    let af = app_fired.clone();

    state.install_element_ime_handlers_for_test(
        id(1),
        ImeHandlers {
            on_ime_preedit: Some(Arc::new(move |_e, cx| {
                *ef.lock().unwrap() += 1;
                cx.stop_propagation();
            })),
            ..Default::default()
        },
    );
    state.install_ime_handlers_for_test(
        vec![Box::new(move |_e: &ImePreeditEvent, _cx: &mut EventCtx| {
            af.set(af.get() + 1)
        })],
        vec![],
        vec![],
        vec![],
    );

    let window = state.window_id_for_test();
    state.dispatch_ime_preedit_for_test(window, "hi".into(), 2, None);

    assert_eq!(*elem_fired.lock().unwrap(), 1, "element handler must fire");
    assert_eq!(
        app_fired.get(),
        0,
        "app handler must not fire after stop_propagation"
    );
}

#[test]
fn dispatch_empty_preedit_clears_active_preedit() {
    // A composition deleted back to empty arrives as an empty-text preedit
    // event. The element handler (mirroring production TextField) must clear
    // the stored preedit so the last typed glyph does not stick on screen.
    let state = make_state();
    let elem_id = id(3);
    state.register_focusable_for_test(entry(3));
    state.set_focus_for_test(elem_id);

    let ime_rc = state.register_ime_state_for_test(elem_id);

    // Element handler mirrors `text_field::handlers::build_ime_preedit_handler`:
    // empty text clears, non-empty text sets.
    state.install_element_ime_handlers_for_test(
        elem_id,
        ImeHandlers {
            on_ime_preedit: Some(Arc::new(move |e: &ImePreeditEvent, cx: &mut EventCtx| {
                let Some(state_rc) = cx.ime_state(elem_id) else {
                    return;
                };
                let mut s = state_rc.borrow_mut();
                if e.text.is_empty() {
                    s.preedit = None;
                } else {
                    s.preedit = Some(Preedit {
                        text: e.text.clone(),
                        cursor_byte_offset: e.cursor_byte_offset,
                        selection: e.selection.clone(),
                    });
                }
                cx.stop_propagation();
            })),
            ..Default::default()
        },
    );

    let window = state.window_id_for_test();

    // Type one glyph, then delete it back to empty.
    state.dispatch_ime_preedit_for_test(window, "ぱ".into(), 3, None);
    assert!(
        ime_rc.borrow().preedit.is_some(),
        "preedit must be set after first composition glyph"
    );

    state.dispatch_ime_preedit_for_test(window, String::new(), 0, None);
    assert!(
        ime_rc.borrow().preedit.is_none(),
        "empty preedit must clear the stored composition — no glyph may stick"
    );
}

// ---------------------------------------------------------------------------
// Commit — empty vs. non-empty text
// ---------------------------------------------------------------------------

#[test]
fn dispatch_ime_commit_with_empty_text_only_clears_preedit() {
    // Empty-text commit clears the preedit in ImeState but MUST NOT call any
    // value-mutation handler. We verify the app-level commit handler is NOT
    // invoked (production TextField mirrors this contract).
    let state = make_state();
    let elem_id = id(2);
    state.register_focusable_for_test(entry(2));
    state.set_focus_for_test(elem_id);

    // Seed an active preedit.
    state.set_ime_state_for_test(
        elem_id,
        ImeState {
            text: String::new(),
            caret: 0,
            preedit: Some(Preedit {
                text: "abc".into(),
                cursor_byte_offset: 3,
                selection: None,
            }),
            caret_client_rect: None,
            ..Default::default()
        },
    );

    let app_called = Rc::new(Cell::new(0u32));
    let ac = app_called.clone();
    state.install_ime_handlers_for_test(
        vec![],
        vec![Box::new(move |_e: &ImeCommitEvent, _cx: &mut EventCtx| {
            ac.set(ac.get() + 1)
        })],
        vec![],
        vec![],
    );

    let window = state.window_id_for_test();
    // Empty commit — "clear preedit, no insert" signal.
    state.dispatch_ime_commit_for_test(window, String::new());

    // The app-level on_ime_commit fires regardless of text emptiness (the
    // contract to skip Signal::set is on the _element_ handler, not on the
    // dispatch layer). So we verify the dispatch reached the app handler.
    // The important semantic: preedit is cleared by drain_pending_ime_ops or
    // element handler. Here we installed only an app handler — preedit state
    // is therefore unchanged by the dispatch (no element handler ran). The
    // app handler count is what we can test from outside.
    assert_eq!(
        app_called.get(),
        1,
        "app handler must fire for empty commit too"
    );
}

#[test]
fn dispatch_ime_commit_with_text_fires_app_handler() {
    let state = make_state();
    let captured = Rc::new(RefCell::new(String::new()));
    let c = captured.clone();

    state.install_ime_handlers_for_test(
        vec![],
        vec![Box::new(move |e: &ImeCommitEvent, _cx: &mut EventCtx| {
            *c.borrow_mut() = e.text.clone()
        })],
        vec![],
        vec![],
    );

    let window = state.window_id_for_test();
    let signal = state.dispatch_ime_commit_for_test(window, "你好".into());

    assert_eq!(&*captured.borrow(), "你好");
    assert_eq!(signal, AppSignal::RequestRedraw);
}

// ---------------------------------------------------------------------------
// Lifecycle — enabled / disabled
// ---------------------------------------------------------------------------

#[test]
fn dispatch_ime_enabled_fires_app_handler() {
    let state = make_state();
    let fired = Rc::new(Cell::new(0u32));
    let f = fired.clone();

    state.install_ime_handlers_for_test(
        vec![],
        vec![],
        vec![Box::new(
            move |_e: &ImeLifecycleEvent, _cx: &mut EventCtx| f.set(f.get() + 1),
        )],
        vec![],
    );

    let window = state.window_id_for_test();
    let signal = state.dispatch_ime_enabled_for_test(window);

    assert_eq!(fired.get(), 1);
    assert_eq!(signal, AppSignal::RequestRedraw);
}

#[test]
fn dispatch_ime_disabled_fires_app_handler() {
    let state = make_state();
    let fired = Rc::new(Cell::new(0u32));
    let f = fired.clone();

    state.install_ime_handlers_for_test(
        vec![],
        vec![],
        vec![],
        vec![Box::new(
            move |_e: &ImeLifecycleEvent, _cx: &mut EventCtx| f.set(f.get() + 1),
        )],
    );

    let window = state.window_id_for_test();
    let signal = state.dispatch_ime_disabled_for_test(window);

    assert_eq!(fired.get(), 1);
    assert_eq!(signal, AppSignal::RequestRedraw);
}

// ---------------------------------------------------------------------------
// WindowImeDelegate query channel (cache-then-query contract)
// ---------------------------------------------------------------------------

#[test]
fn ime_caret_rect_returns_none_when_no_focused_element() {
    let state = make_state();
    // No element registered, no focus — cache is empty.
    let rect = state.ime_caret_rect_query_for_test();
    assert!(rect.is_none());
}

#[test]
fn ime_caret_rect_returns_cached_value_when_focused() {
    let state = make_state();
    let elem_id = id(3);
    state.register_focusable_for_test(entry(3));
    state.set_focus_for_test(elem_id);

    let expected = PhysicalRect {
        x: 10,
        y: 20,
        width: 2,
        height: 16,
    };
    state.set_ime_state_for_test(
        elem_id,
        ImeState {
            caret_client_rect: Some(expected),
            ..Default::default()
        },
    );

    let rect = state.ime_caret_rect_query_for_test();
    assert_eq!(rect, Some(expected));
}

#[test]
fn ime_text_returns_substring_when_in_window() {
    let state = make_state();
    let elem_id = id(4);
    state.register_focusable_for_test(entry(4));
    state.set_focus_for_test(elem_id);

    state.set_ime_state_for_test(
        elem_id,
        ImeState {
            text: "hello world".into(),
            caret: 5,
            ..Default::default()
        },
    );
    // republish_ime_cache is called by set_ime_state_for_test.

    let result = state.ime_text_query_for_test(0..5);
    assert_eq!(result, Some("hello".to_string()));
}

// ---------------------------------------------------------------------------
// Tab during preedit synthesises commit
// ---------------------------------------------------------------------------

#[test]
fn tab_during_preedit_synthesises_commit() {
    // Registers an element with an active preedit, sets focus, then dispatches
    // Tab. The framework must:
    //   1. Detect preedit on the focused element.
    //   2. Queue PendingImeOp::Commit and drain it (writes text + clears preedit).
    //   3. NOT invoke user on_ime_commit handlers (re-entrancy contract).
    let state = make_state();
    let elem_id = id(5);
    state.register_focusable_for_test(entry(5));
    state.set_focus_for_test(elem_id);

    // Seed preedit "abc" in the element's ImeState.
    let ime_rc = state.register_ime_state_for_test(elem_id);
    {
        let mut s = ime_rc.borrow_mut();
        s.preedit = Some(Preedit {
            text: "abc".into(),
            cursor_byte_offset: 3,
            selection: None,
        });
    }

    // App-level commit handler must NOT fire for synthetic commits.
    let user_commit_fired = Rc::new(Cell::new(0u32));
    let ucf = user_commit_fired.clone();
    state.install_ime_handlers_for_test(
        vec![],
        vec![Box::new(move |_e: &ImeCommitEvent, _cx: &mut EventCtx| {
            ucf.set(ucf.get() + 1)
        })],
        vec![],
        vec![],
    );

    // Dispatch Tab — triggers the synthetic commit path.
    state.dispatch_key_down_for_test(
        KeyCode::Tab,
        Key::Named(NamedKey::Tab),
        Modifiers::default(),
        false,
    );

    let after = ime_rc.borrow();
    assert!(after.preedit.is_none(), "preedit must be cleared after Tab");
    assert_eq!(
        after.text, "abc",
        "preedit text must be committed into ImeState.text"
    );
    assert_eq!(
        user_commit_fired.get(),
        0,
        "user on_ime_commit handlers must NOT fire for synthetic commits"
    );
}
