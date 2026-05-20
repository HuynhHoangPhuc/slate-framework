//! Headless integration tests for `TextArea` double-click word-select and
//! triple-click line-select.
//!
//! The `mouse.rs` unit tests cover the pure click-count state machine
//! (`next_click_count`) and `word.rs` covers `word_range_at`; neither goes
//! through the real `on_mouse_down` closure. These tests drive the **production**
//! handlers via `AppState::dispatch_mouse_down`, with a real `MultilineLayout`
//! shaped by the platform text backend seeded onto `ImeState.last_layout` (the
//! field `paint` populates at runtime). Consecutive dispatches carry
//! near-instant timestamps at the same position, so the framework-side
//! click-count detector promotes them to a double / triple click without any
//! platform `clickCount`.
//!
//! Windows + `test-hooks`-gated for the same reason as the other dispatch tests:
//! the live platform text backend + `DefaultPlatform::new()` off the main thread.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::rc::Rc;

use slate_framework::app_state::AppState;
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::elements::text_area::build_mouse_handlers_for_test;
use slate_framework::event::{Modifiers, MouseButton};
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::focus::FocusableEntry;
use slate_framework::hit_test::HitRegion;
use slate_framework::ime::ImeState;
use slate_framework::text_system::TextSystem;
use slate_framework::types::{Bounds, ElementId, Point, Size};
use slate_framework::view::{IntoAny, View};
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
        title: "slate-textarea-mouse-test".into(),
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
/// line0 "alpha" bytes 0..6 (covers '\n'), line1 "beta" 6..11, line2 "gamma".
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

/// Focused TextArea wired into `AppState`: focusable + ime-registered + the real
/// mouse handlers + a hit region at `bounds`. Seeds text, layout, and paint
/// origin (0,0) so a window click maps directly to element-local coords.
fn setup() -> (Rc<AppState<NoopView>>, Rc<std::cell::RefCell<ImeState>>) {
    let state = make_state();
    let elem = ElementId::from_raw(20);
    state.register_focusable_for_test(FocusableEntry {
        id: elem,
        tab_index: 0,
        focus_ring: true,
    });
    state.set_focus_for_test(elem);

    let ime_rc = state.register_ime_state_for_test(elem);
    let layout = Rc::new(three_line_layout());
    {
        let mut s = ime_rc.borrow_mut();
        s.text = "alpha\nbeta\ngamma".to_string();
        s.caret = 0;
        s.last_layout = Some(layout.clone());
        s.paint_origin_x = 0.0;
        s.paint_origin_y = 0.0;
    }
    state.republish_ime_cache_for_test();

    state.install_element_mouse_handlers_for_test(elem, build_mouse_handlers_for_test());
    state.hit_test_list.borrow_mut().push(HitRegion::new(
        elem,
        Bounds {
            origin: Point::new(0.0, 0.0),
            size: Size::new(1000.0, layout.total_height_lpx),
        },
        0,
    ));
    (state, ime_rc)
}

fn click(state: &Rc<AppState<NoopView>>, x: f32, y: f32) {
    state.dispatch_mouse_down_for_test((x, y), MouseButton::Left, Modifiers::default());
}

#[test]
fn single_click_places_collapsed_caret() {
    let (state, ime_rc) = setup();
    // A point inside line0 "alpha": small x, y in the first line's band.
    click(&state, 12.0, 4.0);
    let s = ime_rc.borrow();
    // One click → caret == anchor (collapsed); both inside the word, not snapped.
    assert_eq!(s.click_count, 1);
    assert_eq!(Some(s.caret), s.selection_anchor, "single click is collapsed");
    assert!(s.caret < 5, "caret lands inside 'alpha' (bytes 0..5)");
}

#[test]
fn double_click_selects_the_word() {
    let (state, ime_rc) = setup();
    // Two clicks at the same point → the second promotes to a double click and
    // snaps the selection to the Unicode word "alpha" (bytes 0..5).
    click(&state, 12.0, 4.0);
    click(&state, 12.0, 4.0);
    let s = ime_rc.borrow();
    assert_eq!(s.click_count, 2, "second click is a double");
    assert_eq!(s.selection_anchor, Some(0), "word anchor at 'alpha' start");
    assert_eq!(s.caret, 5, "word caret at 'alpha' end");
}

#[test]
fn triple_click_selects_the_visual_line() {
    let (state, ime_rc) = setup();
    // Three clicks → line select: line0 "alpha" spans bytes 0..6 (the '\n' is
    // folded into byte_end, so the terminator is selected too).
    click(&state, 12.0, 4.0);
    click(&state, 12.0, 4.0);
    click(&state, 12.0, 4.0);
    let s = ime_rc.borrow();
    assert_eq!(s.click_count, 3, "third click is a triple");
    assert_eq!(s.selection_anchor, Some(0), "line anchor at line0 start");
    assert_eq!(s.caret, 6, "line caret at line0 end (past the '\\n')");
}

#[test]
fn fourth_click_wraps_back_to_caret() {
    let (state, ime_rc) = setup();
    for _ in 0..4 {
        click(&state, 12.0, 4.0);
    }
    let s = ime_rc.borrow();
    // 1→2→3→1: the fourth click re-places a collapsed caret rather than selecting.
    assert_eq!(s.click_count, 1, "fourth click wraps to single");
    assert_eq!(Some(s.caret), s.selection_anchor, "wrapped click is collapsed");
}

#[test]
fn multi_click_run_survives_capture_release() {
    // Production sequence on Win32: down → up → down. Before the platform fix,
    // our own `ReleaseCapture()` triggered WM_CAPTURECHANGED → Event::CaptureLost
    // → clear_drag_flags, zeroing click_count between the two downs so a real
    // double-click was always counted as two singles. The platform now suppresses
    // the spurious dispatch; this test locks the post-fix invariant: a mouse-up
    // between two same-position clicks must not break the multi-click run.
    let (state, ime_rc) = setup();
    click(&state, 12.0, 4.0);
    state.dispatch_mouse_up_for_test((12.0, 4.0), MouseButton::Left, Modifiers::default());
    click(&state, 12.0, 4.0);
    let s = ime_rc.borrow();
    assert_eq!(s.click_count, 2, "down→up→down preserves the multi-click counter");
    assert_eq!(s.selection_anchor, Some(0), "word anchor at 'alpha' start");
    assert_eq!(s.caret, 5, "word caret at 'alpha' end");
}

#[test]
fn real_capture_loss_still_resets_multi_click_run() {
    // Counterpart to multi_click_run_survives_capture_release: when capture is
    // genuinely lost (alt-tab, modal popup, sibling SetCapture), the framework
    // must still drop the multi-click counter so a click seconds later on the
    // restored window is a fresh single, not a continuation.
    let (state, ime_rc) = setup();
    click(&state, 12.0, 4.0);
    state.dispatch_mouse_up_for_test((12.0, 4.0), MouseButton::Left, Modifiers::default());
    let _ = state.dispatch_capture_lost_for_test();
    click(&state, 12.0, 4.0);
    let s = ime_rc.borrow();
    assert_eq!(s.click_count, 1, "real CaptureLost resets the multi-click counter");
    assert_eq!(Some(s.caret), s.selection_anchor, "single click is collapsed");
}

#[test]
fn double_click_on_second_line_selects_that_word() {
    let (state, ime_rc) = setup();
    // y in line1's band ("beta", bytes 6..10). line_height is uniform; line1's
    // band starts at total_height/3. Click low enough to land on line1.
    let y = ime_rc.borrow().last_layout.as_ref().unwrap().line_height_lpx * 1.5;
    click(&state, 8.0, y);
    click(&state, 8.0, y);
    let s = ime_rc.borrow();
    assert_eq!(s.click_count, 2);
    assert_eq!(s.selection_anchor, Some(6), "word anchor at 'beta' start");
    assert_eq!(s.caret, 10, "word caret at 'beta' end");
}
