//! Integration tests for TextField-shape IME behaviour (Phase 9c).
//!
//! Rather than constructing a full View tree (which would require the complete
//! prepaint walk), these tests simulate TextField's on_ime_preedit /
//! on_ime_commit handler closures directly against `AppState` dispatch.
//! The grapheme math is already covered by the 16 unit tests inside
//! `elements/text_field/grapheme.rs`; this file locks in the handler-level
//! behaviour observable from outside the crate.
//!
//! Windows-only for the same reason as `keyboard_dispatch.rs`.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use slate_framework::app_state::{AppSignal, AppState};
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::event::{ImeHandlers, ImeCommitEvent, ImePreeditEvent};
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::focus::FocusableEntry;
use slate_framework::ime::{ImeState, Preedit};
use slate_framework::types::ElementId;
use slate_framework::view::{IntoAny, View};
use slate_framework::EventCtx;
use slate_platform::{DefaultPlatform, PhysicalRect, Platform, WindowOptions, wake_run_loop};

// ---------------------------------------------------------------------------
// Shared helpers (mirrors ime_dispatch.rs)
// ---------------------------------------------------------------------------

struct NoopView;

impl View for NoopView {
    fn render(&mut self) -> AnyElement {
        Div::new().into_any()
    }
}

fn make_state() -> Rc<AppState<NoopView>> {
    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-textfield-caret-test".into(),
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

// ---------------------------------------------------------------------------
// Smoke: TextField public API compiles (no view-tree required)
// ---------------------------------------------------------------------------

#[test]
fn text_field_module_smoke() {
    use slate_framework::elements::{TextField, TextFieldStyle};
    use slate_reactive::Runtime;

    // Runtime::new() returns Arc<Runtime> directly.
    let rt = Runtime::new();
    let signal = slate_reactive::Signal::new(rt, String::new());
    // Verify the builder chain and default style compile.
    let _tf = TextField::new(signal).style(TextFieldStyle::default());
}

// ---------------------------------------------------------------------------
// IME preedit → commit with Signal-mirroring handler
// ---------------------------------------------------------------------------

/// Build an `on_ime_preedit` handler closure that mirrors TextField's inner
/// logic: write the preedit into `ImeState` without touching the committed
/// text, then stop propagation so the app handler doesn't also fire.
fn make_preedit_handler(
    ime_rc: Rc<RefCell<ImeState>>,
) -> Arc<dyn Fn(&ImePreeditEvent, &mut EventCtx) + Send + Sync + 'static> {
    Arc::new(move |e: &ImePreeditEvent, cx: &mut EventCtx| {
        let mut s = ime_rc.borrow_mut();
        s.preedit = Some(Preedit {
            text: e.text.clone(),
            cursor_byte_offset: e.cursor_byte_offset,
            selection: e.selection.clone(),
        });
        cx.stop_propagation();
    })
}

/// Build an `on_ime_commit` handler closure that mirrors TextField's inner
/// logic:
///   - always clears preedit
///   - if `text` is non-empty: inserts at caret + advances caret + calls `signal_setter`
///   - if `text` is empty: only clears preedit (no-insert branch)
fn make_commit_handler(
    ime_rc: Rc<RefCell<ImeState>>,
    signal_setter: Rc<dyn Fn(String)>,
) -> Arc<dyn Fn(&ImeCommitEvent, &mut EventCtx) + Send + Sync + 'static> {
    Arc::new(move |e: &ImeCommitEvent, cx: &mut EventCtx| {
        let mut s = ime_rc.borrow_mut();
        s.preedit = None;
        if !e.text.is_empty() {
            let caret = s.caret;
            s.text.insert_str(caret, &e.text);
            s.caret = caret + e.text.len();
            let new_text = s.text.clone();
            drop(s); // release borrow before calling signal_setter
            signal_setter(new_text);
        }
        cx.stop_propagation();
    })
}

#[test]
fn ime_preedit_followed_by_commit_updates_signal() {
    let state = make_state();
    let elem_id = id(10);
    state.register_focusable_for_test(entry(10));
    state.set_focus_for_test(elem_id);

    // Shared ImeState for handlers to read/write.
    let ime_rc = state.register_ime_state_for_test(elem_id);

    // Simple in-test signal: just a RefCell<String>.
    let signal_value: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let sv = signal_value.clone();
    let setter: Rc<dyn Fn(String)> = Rc::new(move |s: String| {
        *sv.borrow_mut() = s;
    });

    state.install_element_ime_handlers_for_test(
        elem_id,
        ImeHandlers {
            on_ime_preedit: Some(make_preedit_handler(ime_rc.clone())),
            on_ime_commit: Some(make_commit_handler(ime_rc.clone(), setter)),
            ..Default::default()
        },
    );

    let window = state.window_id_for_test();

    // --- Step 1: preedit arrives ---
    let sig = state.dispatch_ime_preedit_for_test(window, "你好".into(), 6, None);
    assert_eq!(sig, AppSignal::RequestRedraw);
    {
        let s = ime_rc.borrow();
        assert!(s.preedit.is_some(), "preedit must be set after dispatch");
        assert_eq!(s.preedit.as_ref().unwrap().text, "你好");
        assert!(s.text.is_empty(), "committed text must be unchanged during preedit");
    }

    // --- Step 2: commit finalises ---
    let sig = state.dispatch_ime_commit_for_test(window, "你好".into());
    assert_eq!(sig, AppSignal::RequestRedraw);
    {
        let s = ime_rc.borrow();
        assert!(s.preedit.is_none(), "preedit must be cleared after commit");
        assert_eq!(s.text, "你好", "committed text must contain the commit payload");
    }
    assert_eq!(&*signal_value.borrow(), "你好", "signal must reflect committed text");
}

// ---------------------------------------------------------------------------
// Empty commit only clears preedit — does NOT call setter
// ---------------------------------------------------------------------------

#[test]
fn empty_commit_only_clears_preedit() {
    let state = make_state();
    let elem_id = id(11);
    state.register_focusable_for_test(entry(11));
    state.set_focus_for_test(elem_id);

    let ime_rc = state.register_ime_state_for_test(elem_id);

    // Seed a non-empty preedit.
    {
        let mut s = ime_rc.borrow_mut();
        s.preedit = Some(Preedit {
            text: "abc".into(),
            cursor_byte_offset: 3,
            selection: None,
        });
    }

    let setter_called = Rc::new(Cell::new(0u32));
    let sc = setter_called.clone();
    let setter: Rc<dyn Fn(String)> = Rc::new(move |_s: String| {
        sc.set(sc.get() + 1);
    });

    state.install_element_ime_handlers_for_test(
        elem_id,
        ImeHandlers {
            on_ime_preedit: Some(make_preedit_handler(ime_rc.clone())),
            on_ime_commit: Some(make_commit_handler(ime_rc.clone(), setter)),
            ..Default::default()
        },
    );

    let window = state.window_id_for_test();

    // Empty commit — macOS `unmarkText` / cancel composition.
    state.dispatch_ime_commit_for_test(window, String::new());

    let s = ime_rc.borrow();
    assert!(s.preedit.is_none(), "preedit must be cleared by empty commit");
    assert!(s.text.is_empty(), "committed text must NOT be mutated by empty commit");
    assert_eq!(setter_called.get(), 0, "setter must NOT be called for empty commit");
}

// ---------------------------------------------------------------------------
// Multiple sequential commits accumulate in signal
// ---------------------------------------------------------------------------

#[test]
fn sequential_commits_accumulate() {
    let state = make_state();
    let elem_id = id(12);
    state.register_focusable_for_test(entry(12));
    state.set_focus_for_test(elem_id);

    let ime_rc = state.register_ime_state_for_test(elem_id);

    let signal_value: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
    let sv = signal_value.clone();
    let setter: Rc<dyn Fn(String)> = Rc::new(move |s: String| {
        *sv.borrow_mut() = s;
    });

    state.install_element_ime_handlers_for_test(
        elem_id,
        ImeHandlers {
            on_ime_preedit: Some(make_preedit_handler(ime_rc.clone())),
            on_ime_commit: Some(make_commit_handler(ime_rc.clone(), setter)),
            ..Default::default()
        },
    );

    let window = state.window_id_for_test();

    state.dispatch_ime_commit_for_test(window, "Hello".into());
    state.dispatch_ime_commit_for_test(window, " ".into());
    state.dispatch_ime_commit_for_test(window, "世界".into());

    assert_eq!(&*signal_value.borrow(), "Hello 世界");
    assert_eq!(ime_rc.borrow().text, "Hello 世界");
}

// ---------------------------------------------------------------------------
// Caret rect populated in ImeState and readable via test hook
// ---------------------------------------------------------------------------

#[test]
fn set_ime_state_caret_rect_readable_via_query() {
    let state = make_state();
    let elem_id = id(13);
    state.register_focusable_for_test(entry(13));
    state.set_focus_for_test(elem_id);

    let rect = PhysicalRect { x: 5, y: 10, width: 2, height: 18 };
    state.set_ime_state_for_test(
        elem_id,
        ImeState {
            caret_screen_rect: rect,
            ..Default::default()
        },
    );

    assert_eq!(state.ime_caret_rect_query_for_test(), Some(rect));
}
