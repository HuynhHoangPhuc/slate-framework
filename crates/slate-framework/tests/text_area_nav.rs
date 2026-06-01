//! Cross-platform integration tests for the multi-line `TextArea` key handler.
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
//! These suites construct the real platform (`DefaultPlatform::new()`) plus a
//! 1×1 invisible window, which on macOS requires the process main thread for the
//! `MainThreadMarker`. `cargo test` runs each suite on a worker thread, so this
//! file is built as a `harness = false` `[[test]]` target: cargo runs its
//! `fn main` directly on the process main thread, satisfying AppKit on macOS and
//! working identically on Windows. The `required-features = ["test-hooks"]`
//! Cargo entry gates the build so the `*_for_test` hooks are always present.

use std::rc::Rc;

use slate_framework::app_state::AppState;
use slate_framework::app_state::window_state::WindowState;
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
use slate_platform::{DefaultPlatform, Platform, Window, WindowId, WindowOptions, wake_run_loop};
use slate_reactive::{Runtime, Signal};

/// One named runner case: a label and the check fn that returns `Err` on failure.
type Case = (&'static str, fn() -> Result<(), String>);

/// `assert_eq!` analogue for the `harness = false` runner, where a failure is a
/// returned `Err` rather than a panic. Returns both operands on mismatch.
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
        title: "slate-textarea-nav-test".into(),
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
fn setup(caret: usize) -> (Rc<AppState>, WindowId, Rc<std::cell::RefCell<ImeState>>) {
    let (state, win) = make_state();
    let elem = ElementId::from_raw(20);
    state.register_focusable_for_test(
        win,
        FocusableEntry {
            id: elem,
            tab_index: 0,
            focus_ring: true,
        },
    );
    state.set_focus_for_test(win, elem);

    let ime_rc = state.register_ime_state_for_test(win, elem);
    {
        let mut s = ime_rc.borrow_mut();
        s.text = "alpha\nbeta\ngamma".to_string();
        s.caret = caret;
        s.last_layout = Some(Rc::new(three_line_layout()));
    }
    state.republish_ime_cache_for_test(win);

    // The handler's bound signal: ArrowUp/Down/Home/End never call `set`, so a
    // throwaway runtime-backed signal suffices.
    let rt = Runtime::new();
    let signal = Signal::new(rt, String::new());
    state.install_element_key_handlers_for_test(
        win,
        elem,
        KeyHandlers {
            on_key_down: Some(build_key_down_handler_for_test(signal)),
            on_key_up: None,
            on_text_input: None,
        },
    );
    (state, win, ime_rc)
}

fn press(state: &Rc<AppState>, win: WindowId, code: KeyCode, named: NamedKey) {
    state.dispatch_key_down_for_test(win, code, Key::Named(named), Modifiers::default(), false);
}

/// Smoke proof for the `harness = false` approach: the real platform + a 1×1
/// invisible window + `AppState` must construct on this thread. On macOS this
/// only succeeds on the process main thread (AppKit's `MainThreadMarker`); if
/// cargo ran `main` off the main thread it would abort here.
fn check_platform_and_window_construct() -> Result<(), String> {
    let _state = make_state();
    Ok(())
}

fn check_arrow_down_through_real_handler_moves_to_next_line_no_panic() -> Result<(), String> {
    // Regression guard for the edition-2024 double-borrow in the ↑/↓ arm.
    let (state, win, ime_rc) = setup(0); // caret at document start, line 0, x = 0
    press(&state, win, KeyCode::ArrowDown, NamedKey::ArrowDown);
    let s = ime_rc.borrow();
    // Line 1 ("beta") starts at byte 6 ("alpha\n"); x=0 lands at its start.
    ensure_eq!(s.caret, 6, "↓ from line0 col0 lands at start of line1");
    ensure_eq!(s.desired_x, Some(0.0), "↓ seeds the sticky column");
    Ok(())
}

fn check_arrow_up_at_first_line_clamps_no_panic() -> Result<(), String> {
    let (state, win, ime_rc) = setup(3); // mid line0
    press(&state, win, KeyCode::ArrowUp, NamedKey::ArrowUp);
    let s = ime_rc.borrow();
    ensure_eq!(s.caret, 0, "↑ on the first line clamps to line start");
    Ok(())
}

fn check_end_through_real_handler_is_visual_line_relative_no_panic() -> Result<(), String> {
    // Regression guard for the double-borrow in the Home/End arm.
    let (state, win, ime_rc) = setup(6); // start of line1 "beta"
    press(&state, win, KeyCode::End, NamedKey::End);
    let s = ime_rc.borrow();
    // "beta" occupies bytes 6..10; End lands at 10 (before the '\n'), not doc end.
    ensure_eq!(s.caret, 10, "End is visual-line relative");
    Ok(())
}

fn check_home_through_real_handler_is_visual_line_relative_no_panic() -> Result<(), String> {
    let (state, win, ime_rc) = setup(9); // mid line1 "beta"
    press(&state, win, KeyCode::Home, NamedKey::Home);
    let s = ime_rc.borrow();
    ensure_eq!(s.caret, 6, "Home lands at line1 start, not document start");
    Ok(())
}

fn main() {
    let cases: &[Case] = &[
        (
            "platform_and_window_construct",
            check_platform_and_window_construct,
        ),
        (
            "arrow_down_through_real_handler_moves_to_next_line_no_panic",
            check_arrow_down_through_real_handler_moves_to_next_line_no_panic,
        ),
        (
            "arrow_up_at_first_line_clamps_no_panic",
            check_arrow_up_at_first_line_clamps_no_panic,
        ),
        (
            "end_through_real_handler_is_visual_line_relative_no_panic",
            check_end_through_real_handler_is_visual_line_relative_no_panic,
        ),
        (
            "home_through_real_handler_is_visual_line_relative_no_panic",
            check_home_through_real_handler_is_visual_line_relative_no_panic,
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
