//! Integration tests for TextField mouse handlers.
//!
//! Mirrors the production handler logic against `AppState` dispatch to lock in
//! click-to-position + drag-select state transitions. The byte-x math is
//! covered by golden tests inside `slate_text::glyph_geometry`; this file
//! exercises the dispatch wiring + `ImeState` mutations observed from outside
//! the crate.
//!
//! Windows + `test-hooks`-gated for the same reason as the other dispatch
//! integration tests.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::rc::Rc;
use std::sync::Arc;

use slate_framework::app_state::{AppSignal, AppState};
use slate_framework::app_state::window_state::WindowState;
use slate_framework::element::AnyElement;
use slate_framework::elements::Div;
use slate_framework::event::{EventCtx, Modifiers, MouseButton, MouseEvent, MouseHandlers};
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::focus::FocusableEntry;
use slate_framework::hit_test::HitRegion;
use slate_framework::ime::Preedit;
use slate_framework::types::{Bounds, ElementId, Point, Size};
use slate_framework::view::{IntoAny, View};
use slate_platform::{DefaultPlatform, Platform, Window, WindowId, WindowOptions, wake_run_loop};
use slate_text::{ShapedGlyph, ShapedLine, byte_at_pixel_x, types::FontId};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

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
        title: "slate-textfield-mouse-test".into(),
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

/// Build a synthetic ShapedLine for `text` where every byte is one cluster
/// of advance `adv`. Test-only — production shaping is exercised in
/// `slate-text` unit tests.
fn synth_shaped(text: &str, adv: f32) -> ShapedLine {
    let glyphs: Vec<ShapedGlyph> = text
        .char_indices()
        .enumerate()
        .map(|(i, (b, _))| ShapedGlyph {
            glyph_id: 1,
            font_id: FontId::PRIMARY,
            font_handle: slate_text::FontHandle::default(),
            x_advance_lpx: adv,
            position_lpx: [i as f32 * adv, 0.0],
            cluster: b as u32,
            direction: slate_text::Direction::Ltr,
        })
        .collect();
    let width = adv * glyphs.len() as f32;
    ShapedLine {
        glyphs,
        width_lpx: width,
        ascent_lpx: 12.0,
        descent_lpx: -3.0,
        y_offset_lpx: 0.0,
        base_direction: slate_text::Direction::Ltr,
        runs: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// Handler mirrors (match `text_field::handlers::build_mouse_*` semantically;
// production builders are `pub(super)`, so tests rebuild the closures inline).
// ---------------------------------------------------------------------------

type Mouse = Arc<dyn Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static>;

fn mirror_down(target: ElementId) -> Mouse {
    Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
        let Some(state_rc) = cx.ime_state(target) else {
            return;
        };
        let mut state = state_rc.borrow_mut();
        if state.preedit.is_some() {
            return;
        }
        let Some(shaped) = state.last_shaped.clone() else {
            return;
        };
        let local_x = ev.position.0 - state.paint_origin_x;
        let byte = byte_at_pixel_x(&shaped, &state.text, local_x);
        state.caret = byte;
        state.selection_anchor = Some(byte);
        state.dragging = true;
        drop(state);
        cx.stop_propagation();
    })
}

fn mirror_move(target: ElementId) -> Mouse {
    Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
        let Some(state_rc) = cx.ime_state(target) else {
            return;
        };
        let mut state = state_rc.borrow_mut();
        if !state.dragging || state.preedit.is_some() {
            return;
        }
        let Some(shaped) = state.last_shaped.clone() else {
            return;
        };
        let local_x = ev.position.0 - state.paint_origin_x;
        let byte = byte_at_pixel_x(&shaped, &state.text, local_x);
        state.caret = byte;
        drop(state);
        cx.stop_propagation();
    })
}

fn mirror_up(target: ElementId) -> Mouse {
    Arc::new(move |_ev: &MouseEvent, cx: &mut EventCtx| {
        let Some(state_rc) = cx.ime_state(target) else {
            return;
        };
        {
            let mut state = state_rc.borrow_mut();
            state.dragging = false;
            if state.selection_anchor == Some(state.caret) {
                state.selection_anchor = None;
            }
        }
        cx.stop_propagation();
    })
}

/// Wire `id` into AppState as: focusable + ime-registered + mouse handlers +
/// hit region at `bounds`. Returns the shared `Rc<RefCell<ImeState>>`.
fn wire_textfield(
    state: &AppState,
    win: WindowId,
    elem_id: ElementId,
    bounds: Bounds,
) -> Rc<std::cell::RefCell<slate_framework::ime::ImeState>> {
    state.register_focusable_for_test(win, entry(elem_id.0));
    state.set_focus_for_test(win, elem_id);
    let ime_rc = state.register_ime_state_for_test(win, elem_id);
    state.install_element_mouse_handlers_for_test(
        win,
        elem_id,
        MouseHandlers {
            on_mouse_down: Some(mirror_down(elem_id)),
            on_mouse_move: Some(mirror_move(elem_id)),
            on_mouse_up: Some(mirror_up(elem_id)),
        },
    );
    state.push_hit_region_for_test(win, HitRegion::new(elem_id, bounds, 0));
    ime_rc
}

fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds {
    Bounds {
        origin: Point::new(x, y),
        size: Size::new(w, h),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn mouse_down_seeds_caret_and_anchor_at_clicked_byte() {
    let (state, win) = make_state();
    let elem_id = id(20);
    // bounds origin x = 100 so a window click at (110, _) maps to local_x=10.
    let ime_rc = wire_textfield(&state, win, elem_id, bounds(100.0, 50.0, 200.0, 20.0));

    {
        let mut s = ime_rc.borrow_mut();
        s.text = "ABCD".into();
        s.last_shaped = Some(Rc::new(synth_shaped("ABCD", 10.0)));
        s.paint_origin_x = 100.0;
        s.caret = 0;
        s.selection_anchor = None;
        s.dragging = false;
    }

    // local_x = 110 - 100 = 10 → past first glyph midpoint (5), before second midpoint (15) → byte 1.
    state.dispatch_mouse_down_for_test(win, (110.0, 55.0), MouseButton::Left, Modifiers::default());

    let s = ime_rc.borrow();
    assert_eq!(
        s.caret, 1,
        "click at local_x=10 (between mids 5 and 15) → byte 1"
    );
    assert_eq!(
        s.selection_anchor,
        Some(1),
        "anchor seeded at caret on mouse_down"
    );
    assert!(s.dragging, "drag flag set on mouse_down");
}

#[test]
fn mouse_move_extends_caret_keeps_anchor() {
    let (state, win) = make_state();
    let elem_id = id(21);
    let ime_rc = wire_textfield(&state, win, elem_id, bounds(0.0, 0.0, 200.0, 20.0));

    {
        let mut s = ime_rc.borrow_mut();
        s.text = "ABCD".into();
        s.last_shaped = Some(Rc::new(synth_shaped("ABCD", 10.0)));
        s.paint_origin_x = 0.0;
    }

    // local_x=4 is below first cluster midpoint (5) → byte 0.
    state.dispatch_mouse_down_for_test(win, (4.0, 5.0), MouseButton::Left, Modifiers::default());
    {
        let s = ime_rc.borrow();
        assert_eq!(s.caret, 0);
        assert_eq!(s.selection_anchor, Some(0));
        assert!(s.dragging);
    }

    // Move to x=35 (mid of 4th cluster) → snaps trailing to byte 4 (text.len).
    state.dispatch_mouse_move_for_test(win, (35.0, 5.0));
    let s = ime_rc.borrow();
    assert!(s.caret >= 3, "caret advanced under drag (got {})", s.caret);
    assert_eq!(s.selection_anchor, Some(0), "anchor stays put during drag");
    assert!(s.dragging, "still dragging during move");
}

#[test]
fn mouse_up_clears_drag_and_collapses_empty_selection() {
    let (state, win) = make_state();
    let elem_id = id(22);
    let ime_rc = wire_textfield(&state, win, elem_id, bounds(0.0, 0.0, 200.0, 20.0));

    {
        let mut s = ime_rc.borrow_mut();
        s.text = "AB".into();
        s.last_shaped = Some(Rc::new(synth_shaped("AB", 10.0)));
        s.paint_origin_x = 0.0;
    }

    // Down then up at the same position → caret == anchor → selection collapses.
    state.dispatch_mouse_down_for_test(win, (5.0, 5.0), MouseButton::Left, Modifiers::default());
    state.dispatch_mouse_up_for_test(win, (5.0, 5.0), MouseButton::Left, Modifiers::default());

    let s = ime_rc.borrow();
    assert!(!s.dragging, "drag cleared on mouse_up");
    assert_eq!(
        s.selection_anchor, None,
        "anchor cleared when collapsed at caret"
    );
}

#[test]
fn mouse_up_after_drag_keeps_non_empty_selection() {
    let (state, win) = make_state();
    let elem_id = id(23);
    let ime_rc = wire_textfield(&state, win, elem_id, bounds(0.0, 0.0, 200.0, 20.0));

    {
        let mut s = ime_rc.borrow_mut();
        s.text = "ABCD".into();
        s.last_shaped = Some(Rc::new(synth_shaped("ABCD", 10.0)));
        s.paint_origin_x = 0.0;
    }

    // local_x=4 → byte 0 anchor, drag to local_x=35 → byte 4 caret.
    state.dispatch_mouse_down_for_test(win, (4.0, 5.0), MouseButton::Left, Modifiers::default());
    state.dispatch_mouse_move_for_test(win, (35.0, 5.0));
    state.dispatch_mouse_up_for_test(win, (35.0, 5.0), MouseButton::Left, Modifiers::default());

    let s = ime_rc.borrow();
    assert!(!s.dragging, "drag cleared on mouse_up");
    assert_eq!(
        s.selection_anchor,
        Some(0),
        "anchor preserved (selection lives)"
    );
    assert!(s.caret >= 3, "caret advanced during drag (got {})", s.caret);
}

#[test]
fn capture_lost_clears_dragging_so_next_move_is_inert() {
    let (state, win) = make_state();
    let elem_id = id(25);
    let ime_rc = wire_textfield(&state, win, elem_id, bounds(0.0, 0.0, 200.0, 20.0));

    {
        let mut s = ime_rc.borrow_mut();
        s.text = "ABCD".into();
        s.last_shaped = Some(Rc::new(synth_shaped("ABCD", 10.0)));
        s.paint_origin_x = 0.0;
    }

    // Down → dragging=true, anchor seeded at byte 0.
    state.dispatch_mouse_down_for_test(win, (4.0, 5.0), MouseButton::Left, Modifiers::default());
    assert!(ime_rc.borrow().dragging, "drag flag set on mouse_down");

    // OS revokes capture before mouse_up (window lost focus, modal popup, …).
    state.dispatch_capture_lost_for_test(win);
    assert!(
        !ime_rc.borrow().dragging,
        "capture_lost must clear dragging — otherwise next move re-extends selection"
    );

    // Subsequent move with no preceding down must NOT extend the caret.
    let caret_before = ime_rc.borrow().caret;
    state.dispatch_mouse_move_for_test(win, (35.0, 5.0));
    let s = ime_rc.borrow();
    assert_eq!(
        s.caret, caret_before,
        "caret must not move when not dragging"
    );
}

#[test]
fn mouse_down_during_preedit_is_noop() {
    let (state, win) = make_state();
    let elem_id = id(24);
    let ime_rc = wire_textfield(&state, win, elem_id, bounds(0.0, 0.0, 200.0, 20.0));

    {
        let mut s = ime_rc.borrow_mut();
        s.text = "AB".into();
        s.caret = 1;
        s.last_shaped = Some(Rc::new(synth_shaped("AB", 10.0)));
        s.paint_origin_x = 0.0;
        s.preedit = Some(Preedit {
            text: "x".into(),
            cursor_byte_offset: 0,
            selection: None,
        });
    }

    state.dispatch_mouse_down_for_test(win, (25.0, 5.0), MouseButton::Left, Modifiers::default());

    let s = ime_rc.borrow();
    assert_eq!(s.caret, 1, "caret unchanged during preedit");
    assert!(
        s.selection_anchor.is_none(),
        "no anchor seeded during preedit"
    );
    assert!(!s.dragging, "no drag started during preedit");
}

#[test]
fn mouse_move_during_capture_requests_redraw() {
    let (state, win) = make_state();
    let elem_id = id(40);
    let ime_rc = wire_textfield(&state, win, elem_id, bounds(0.0, 0.0, 200.0, 20.0));
    {
        let mut s = ime_rc.borrow_mut();
        s.text = "ABCD".into();
        s.last_shaped = Some(Rc::new(synth_shaped("ABCD", 10.0)));
        s.paint_origin_x = 0.0;
    }
    // mouse_down inside the hit region sets the capture target (button held).
    state.dispatch_mouse_down_for_test(win, (5.0, 5.0), MouseButton::Left, Modifiers::default());
    // A bare move while captured must request a redraw so the render-pass
    // flush_coalesced_move runs and the drag extends the selection. Without
    // this signal, Windows only redraws on the 530 ms blink and the drag stalls.
    assert!(
        matches!(
            state.dispatch_mouse_moved_raw_for_test(win, (35.0, 5.0)),
            AppSignal::RequestRedraw { .. }
        ),
        "drag-in-progress move must request a redraw"
    );
}

#[test]
fn mouse_move_without_capture_is_none() {
    let (state, win) = make_state();
    let elem_id = id(41);
    wire_textfield(&state, win, elem_id, bounds(0.0, 0.0, 200.0, 20.0));
    // No prior mouse_down → no capture target → idle hover must stay silent
    // (returning RequestRedraw here would cause a redraw-storm on every move).
    assert_eq!(
        state.dispatch_mouse_moved_raw_for_test(win, (35.0, 5.0)),
        AppSignal::None,
        "idle hover (no capture) must not request a redraw"
    );
}
