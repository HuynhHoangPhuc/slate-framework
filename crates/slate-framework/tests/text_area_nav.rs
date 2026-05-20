//! Headless integration tests for the multi-line `TextArea` key handler.
//!
//! The `nav.rs` unit tests call `move_vertical` / `move_line_edge` directly;
//! they never go through `build_key_down_handler`, so the handler's own
//! `RefCell` borrow pattern (clone `last_layout`, then `borrow_mut`) is
//! unexercised by them. These tests drive the **real** production closure
//! through `AppState::dispatch_key_down`, with a real `MultilineLayout` shaped
//! by the platform text backend seeded onto `ImeState.last_layout` (the field
//! `paint` populates at runtime). That covers the edition-2024 if-let temporary
//! scope so a double-borrow regression in the ↑/↓ / Home/End arms would panic
//! the test rather than only production.
//!
//! Windows-only for the same reason as the other dispatch tests: the live
//! platform text backend + `DefaultPlatform::new()` off the main thread.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::rc::Rc;

use slate_framework::app_state::AppState;
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::elements::text_area::build_key_down_handler_for_test;
use slate_framework::event::KeyHandlers;
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::focus::FocusableEntry;
use slate_framework::ime::ImeState;
use slate_framework::text_system::TextSystem;
use slate_framework::types::ElementId;
use slate_framework::view::{IntoAny, View};
use slate_framework::{Key, KeyCode, Modifiers, NamedKey};
use slate_reactive::{Runtime, Signal};
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
        title: "slate-textarea-nav-test".into(),
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

/// Shape "alpha\nbeta\ngamma" at a generous width → 3 hard-newline visual lines.
fn three_line_layout() -> slate_text::MultilineLayout {
    let mut text_system = TextSystem::new().expect("create TextSystem");
    let font = text_system
        .load_font_from_bytes(slate_text::TEST_FONT, 14.0, 1.0)
        .expect("load bundled font");
    let doc = text_system
        .shape_document(&font, "alpha\nbeta\ngamma")
        .expect("shape document");
    slate_text::wrap_document(&doc, 1000.0)
}

/// Build a focused TextArea element with a seeded `ImeState` (text + layout)
/// and the real key handler installed. Returns the shared `ImeState` handle.
fn setup(caret: usize) -> (Rc<AppState<NoopView>>, Rc<std::cell::RefCell<ImeState>>) {
    let state = make_state();
    let elem = ElementId::from_raw(20);
    state.register_focusable_for_test(FocusableEntry {
        id: elem,
        tab_index: 0,
        focus_ring: true,
    });
    state.set_focus_for_test(elem);

    let ime_rc = state.register_ime_state_for_test(elem);
    {
        let mut s = ime_rc.borrow_mut();
        s.text = "alpha\nbeta\ngamma".to_string();
        s.caret = caret;
        s.last_layout = Some(Rc::new(three_line_layout()));
    }
    state.republish_ime_cache_for_test();

    // The handler's bound signal: ArrowUp/Down/Home/End never call `set`, so a
    // throwaway runtime-backed signal suffices.
    let rt = Runtime::new();
    let signal = Signal::new(rt, String::new());
    state.install_element_key_handlers_for_test(
        elem,
        KeyHandlers {
            on_key_down: Some(build_key_down_handler_for_test(signal)),
            on_key_up: None,
            on_text_input: None,
        },
    );
    (state, ime_rc)
}

fn press(state: &Rc<AppState<NoopView>>, code: KeyCode, named: NamedKey) {
    state.dispatch_key_down_for_test(code, Key::Named(named), Modifiers::default(), false);
}

#[test]
fn arrow_down_through_real_handler_moves_to_next_line_no_panic() {
    // Regression guard for the edition-2024 double-borrow in the ↑/↓ arm.
    let (state, ime_rc) = setup(0); // caret at document start, line 0, x = 0
    press(&state, KeyCode::ArrowDown, NamedKey::ArrowDown);
    let s = ime_rc.borrow();
    // Line 1 ("beta") starts at byte 6 ("alpha\n"); x=0 lands at its start.
    assert_eq!(s.caret, 6, "↓ from line0 col0 lands at start of line1");
    assert_eq!(s.desired_x, Some(0.0), "↓ seeds the sticky column");
}

#[test]
fn arrow_up_at_first_line_clamps_no_panic() {
    let (state, ime_rc) = setup(3); // mid line0
    press(&state, KeyCode::ArrowUp, NamedKey::ArrowUp);
    let s = ime_rc.borrow();
    assert_eq!(s.caret, 0, "↑ on the first line clamps to line start");
}

#[test]
fn end_through_real_handler_is_visual_line_relative_no_panic() {
    // Regression guard for the double-borrow in the Home/End arm.
    let (state, ime_rc) = setup(6); // start of line1 "beta"
    press(&state, KeyCode::End, NamedKey::End);
    let s = ime_rc.borrow();
    // "beta" occupies bytes 6..10; End lands at 10 (before the '\n'), not doc end.
    assert_eq!(s.caret, 10, "End is visual-line relative");
}

#[test]
fn home_through_real_handler_is_visual_line_relative_no_panic() {
    let (state, ime_rc) = setup(9); // mid line1 "beta"
    press(&state, KeyCode::Home, NamedKey::Home);
    let s = ime_rc.borrow();
    assert_eq!(s.caret, 6, "Home lands at line1 start, not document start");
}
