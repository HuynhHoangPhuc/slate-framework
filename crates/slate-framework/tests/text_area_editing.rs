//! End-to-end editing tests for `TextArea`, driving the **production** key /
//! text-input / IME / clipboard handlers through `AppState::dispatch_*`.
//!
//! Where the per-phase suites isolate one concern (`text_area_nav` = caret
//! motion, `text_area_mouse` = click selection, the `*_edit` unit tests = pure
//! ops), this file exercises the cross-phase flows the way a user produces them:
//! type two lines, select, replace, undo; copy a multi-line selection and paste
//! it back (newlines must survive); commit an IME composition mid-document.
//!
//! Headless caveat: there is no paint pass, so `ImeState.last_layout` (which
//! `paint` refreshes each frame) is stale after an edit. The flows here use only
//! layout-independent handlers (typing, Enter, grapheme motion, clipboard, undo,
//! IME); the one navigation assertion re-seeds the layout exactly as `paint`
//! would, then drives the real Home/End handler.
//!
//! Runs cross-platform as a `harness = false` `[[test]]` (its `fn main` executes
//! on the process main thread, so the real platform + a 1×1 window construct on
//! macOS); `required-features = ["test-hooks"]` gates the build.

use std::cell::RefCell;
use std::rc::Rc;

/// `assert!` analogue for the `harness = false` runner: returns `Err` instead of
/// panicking so the `fn main` runner can report it as a failed case.
macro_rules! ensure {
    ($cond:expr, $($arg:tt)*) => {
        if !$cond {
            return Err(format!($($arg)*));
        }
    };
}

/// `assert_eq!` analogue: returns `Err` with both operands on mismatch.
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

/// One named runner case: a label and the check fn that returns `Err` on failure.
type Case = (&'static str, fn() -> Result<(), String>);

use slate_framework::app_state::AppState;
use slate_framework::app_state::window_state::WindowState;
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::elements::text_area::{
    build_ime_handlers_for_test, build_key_down_handler_for_test, build_text_input_handler_for_test,
};
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
        title: "slate-textarea-editing-test".into(),
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

/// A focused, fully-wired TextArea: the real key + text-input + IME handlers
/// bound to a fresh `Signal<String>`. The signal's runtime is returned so the
/// caller keeps it alive for the test's duration (a dropped runtime would sever
/// `value.set`).
struct Harness {
    state: Rc<AppState>,
    ime: Rc<RefCell<ImeState>>,
    value: Signal<String>,
    window: WindowId,
    _rt: std::sync::Arc<Runtime>,
}

fn harness() -> Harness {
    // Route copy/paste through an in-memory clipboard so the round-trip is
    // deterministic and independent of a working OS pasteboard (headless CI).
    slate_platform::clipboard::install_clipboard_override_for_test();
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

    let ime = state.register_ime_state_for_test(win, elem);
    state.republish_ime_cache_for_test(win);

    let rt = Runtime::new();
    let value = Signal::new(rt.clone(), String::new());
    state.install_element_key_handlers_for_test(
        win,
        elem,
        KeyHandlers {
            on_key_down: Some(build_key_down_handler_for_test(value.clone())),
            on_key_up: None,
            on_text_input: Some(build_text_input_handler_for_test(value.clone())),
        },
    );
    state.install_element_ime_handlers_for_test(
        win,
        elem,
        build_ime_handlers_for_test(value.clone()),
    );

    Harness {
        state,
        ime,
        value,
        window: win,
        _rt: rt,
    }
}

impl Harness {
    fn typ(&self, s: &str) {
        self.state
            .dispatch_text_input_for_test(self.window, s.to_string());
    }
    fn key(&self, code: KeyCode, key: Key, mods: Modifiers) {
        self.state
            .dispatch_key_down_for_test(self.window, code, key, mods, false);
    }
    fn enter(&self) {
        self.key(
            KeyCode::Enter,
            Key::Named(NamedKey::Enter),
            Modifiers::default(),
        );
    }
    fn shift_left(&self) {
        self.key(
            KeyCode::ArrowLeft,
            Key::Named(NamedKey::ArrowLeft),
            Modifiers {
                shift: true,
                ..Default::default()
            },
        );
    }
    fn arrow_right(&self) {
        self.key(
            KeyCode::ArrowRight,
            Key::Named(NamedKey::ArrowRight),
            Modifiers::default(),
        );
    }
    /// Command-modified letter shortcut. The command modifier is Cmd (`meta`)
    /// on macOS and Ctrl elsewhere, matching `is_command_modifier`, so clipboard
    /// and undo shortcuts fire on the platform the test actually runs on.
    fn command(&self, code: KeyCode, ch: &str) {
        let mods = if cfg!(target_os = "macos") {
            Modifiers {
                meta: true,
                ..Default::default()
            }
        } else {
            Modifiers {
                ctrl: true,
                ..Default::default()
            }
        };
        self.key(code, Key::Character(ch.into()), mods);
    }
    fn text(&self) -> String {
        self.value.get_untracked()
    }
    fn caret(&self) -> usize {
        self.ime.borrow().caret
    }
}

/// Shape `text` at a wide width (newlines are the only breaks) and write it onto
/// `ImeState.last_layout`, mirroring what `paint` does each frame so the
/// layout-dependent nav handlers see geometry matching the current buffer.
fn reseed_layout(h: &Harness, text: &str) {
    let mut ts = TextSystem::new().expect("create TextSystem");
    let font = ts
        .load_font_from_bytes(slate_text::TEST_FONT, 14.0, 1.0)
        .expect("load font");
    let doc = ts.shape_document(&font, text).expect("shape");
    let layout = slate_text::wrap_document(&doc, 1000.0);
    h.ime.borrow_mut().last_layout = Some(Rc::new(layout));
}

fn check_type_two_lines_select_replace_then_undo() -> Result<(), String> {
    let h = harness();
    h.typ("a");
    h.typ("b");
    h.enter();
    h.typ("c");
    h.typ("d");
    ensure_eq!(
        h.text(),
        "ab\ncd",
        "Enter inserts a newline between the runs"
    );
    ensure_eq!(h.caret(), 5, "caret after typing 'ab\\ncd'");

    // Select "cd" with two Shift+Left, then type over it.
    h.shift_left();
    h.shift_left();
    ensure_eq!(
        h.ime.borrow().selection_anchor,
        Some(5),
        "selection anchor at byte 5"
    );
    ensure_eq!(h.caret(), 3, "selection spans bytes 3..5 ('cd')");

    h.typ("X");
    ensure_eq!(h.text(), "ab\nX", "typing replaces the selection");
    ensure_eq!(h.caret(), 4, "caret after replacing selection with 'X'");

    // Undo restores the replaced text.
    h.command(KeyCode::KeyZ, "z");
    ensure_eq!(h.text(), "ab\ncd", "undo reverts the replace");
    Ok(())
}

fn check_copy_multiline_selection_and_paste_preserves_newlines() -> Result<(), String> {
    let h = harness();
    h.typ("a");
    h.typ("b");
    h.enter();
    h.typ("c");
    h.typ("d");
    ensure_eq!(h.text(), "ab\ncd", "seeded buffer");

    // Select the whole document (5 Shift+Left from byte 5 → caret 0, anchor 5).
    for _ in 0..5 {
        h.shift_left();
    }
    ensure_eq!(h.caret(), 0, "caret at document start after selecting all");
    ensure_eq!(
        h.ime.borrow().selection_anchor,
        Some(5),
        "anchor at document end"
    );

    // Copy, collapse to the right edge, then paste at the end.
    h.command(KeyCode::KeyC, "c");
    h.arrow_right(); // collapses selection to byte 5 (caret end)
    ensure_eq!(h.caret(), 5, "ArrowRight collapses to byte 5");
    ensure_eq!(
        h.ime.borrow().selection_anchor,
        None,
        "selection cleared on collapse"
    );

    h.command(KeyCode::KeyV, "v");
    ensure_eq!(
        h.text(),
        "ab\ncdab\ncd",
        "multi-line paste keeps the '\\n' (multiline = true)"
    );
    Ok(())
}

fn check_ime_commit_inserts_at_caret_mid_document() -> Result<(), String> {
    let h = harness();
    // Seed "ad" with the caret between 'a' and 'd'.
    {
        let mut s = h.ime.borrow_mut();
        s.text = "ad".to_string();
        s.caret = 1;
        s.seed_undo_baseline();
    }
    h.value.set("ad".to_string());

    // Compose then commit "b": the preedit must not mutate the buffer, the
    // commit inserts at the caret.
    h.state
        .dispatch_ime_preedit_for_test(h.window, "b".into(), 1, None);
    {
        let s = h.ime.borrow();
        ensure_eq!(s.text.as_str(), "ad", "preedit leaves the buffer untouched");
        ensure!(
            s.preedit.is_some(),
            "preedit is recorded during composition"
        );
    }

    h.state.dispatch_ime_commit_for_test(h.window, "b".into());
    ensure_eq!(h.text(), "abd", "commit inserts at the caret mid-document");
    ensure_eq!(h.caret(), 2, "caret advances past the committed text");
    ensure!(
        h.ime.borrow().preedit.is_none(),
        "commit clears the preedit"
    );
    Ok(())
}

fn check_enter_inserts_exactly_one_newline() -> Result<(), String> {
    // Win32 dispatches both KeyDown(Enter) and a subsequent TextInput("\n") for
    // the same physical keystroke. The text-input handler must drop the
    // bare-newline payload so only the KeyDown path's "\n" insertion lands.
    let h = harness();
    h.enter();
    h.typ("\n");
    ensure_eq!(h.text(), "\n", "one Enter press yields exactly one newline");
    ensure_eq!(h.caret(), 1, "caret after the single newline");
    Ok(())
}

fn check_enter_inserts_exactly_one_newline_carriage_return() -> Result<(), String> {
    // Some platforms normalize Enter to "\r" before delivering as text input.
    let h = harness();
    h.enter();
    h.typ("\r");
    ensure_eq!(h.text(), "\n", "bare \\r from text input is also filtered");
    ensure_eq!(h.caret(), 1, "caret after the single newline");
    Ok(())
}

fn check_end_key_after_typing_is_visual_line_relative() -> Result<(), String> {
    let h = harness();
    h.typ("a");
    h.typ("b");
    h.enter();
    h.typ("c");
    h.typ("d"); // "ab\ncd", caret 5

    // Mirror paint's layout cache so the End handler sees current geometry, then
    // move the caret to line0 and press End — it must stop at line0's end (2),
    // not the document end (5).
    reseed_layout(&h, "ab\ncd");
    // Collapse to document start via 5 Shift+Left + Left would be noisy; instead
    // place the caret on line0 directly and assert End is line-relative.
    h.ime.borrow_mut().caret = 0;
    h.key(
        KeyCode::End,
        Key::Named(NamedKey::End),
        Modifiers::default(),
    );
    ensure_eq!(
        h.caret(),
        2,
        "End lands at line0 end ('ab'), not document end"
    );
    Ok(())
}

fn main() {
    let cases: &[Case] = &[
        (
            "type_two_lines_select_replace_then_undo",
            check_type_two_lines_select_replace_then_undo,
        ),
        (
            "copy_multiline_selection_and_paste_preserves_newlines",
            check_copy_multiline_selection_and_paste_preserves_newlines,
        ),
        (
            "ime_commit_inserts_at_caret_mid_document",
            check_ime_commit_inserts_at_caret_mid_document,
        ),
        (
            "enter_inserts_exactly_one_newline",
            check_enter_inserts_exactly_one_newline,
        ),
        (
            "enter_inserts_exactly_one_newline_carriage_return",
            check_enter_inserts_exactly_one_newline_carriage_return,
        ),
        (
            "end_key_after_typing_is_visual_line_relative",
            check_end_key_after_typing_is_visual_line_relative,
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
