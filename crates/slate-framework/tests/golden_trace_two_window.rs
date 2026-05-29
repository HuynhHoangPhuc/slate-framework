//! Two-window companion to `dispatch_golden_trace.rs`.
//!
//! Pins the handler-invocation order when events are interleaved across two
//! live windows. A single `AppState` owns both `WindowState`s; each window
//! has its own handler registries and focus state. The golden trace asserts:
//!
//! - Window A events route only to window A's handlers.
//! - Window B events route only to window B's handlers.
//! - Interleaved sequences produce a stable, ordered trace.
//!
//! Snapshot mechanism: `insta::assert_debug_snapshot!` — same as the
//! single-window fixture. The first run creates the `.snap` file; subsequent
//! runs compare against it. Bless with `cargo insta review` when an
//! intentional dispatch change occurs.
//!
//! Runs cross-platform as a `harness = false` `[[test]]` on the process main
//! thread. Gated on `required-features = ["test-hooks"]`.

#![cfg(all(
    any(target_os = "macos", target_os = "windows"),
    feature = "test-hooks"
))]

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use insta::assert_debug_snapshot;
use slate_framework::app_state::window_state::WindowState;
use slate_framework::app_state::{AppSignal, AppState};
use slate_framework::event::{EventCtx, KeyEvent, KeyHandlers, MouseEvent, MouseHandlers};
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::focus::FocusableEntry;
use slate_framework::hit_test::HitRegion;
use slate_framework::types::{Bounds, ElementId, Point, Size};
use slate_framework::{Key, KeyCode, Modifiers, MouseButton, NamedKey};
use slate_platform::{DefaultPlatform, Platform, Window, WindowId, WindowOptions, wake_run_loop};

type Case = (&'static str, fn() -> Result<(), String>);

// ---------------------------------------------------------------------------
// Harness scaffolding
// ---------------------------------------------------------------------------

/// Construct an `AppState` with two registered test windows.
fn make_two_window_state() -> (Rc<AppState>, WindowId, WindowId) {
    let platform = DefaultPlatform::new();

    let window_a = platform.create_window(WindowOptions {
        title: "slate-golden-trace-two-win-a".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32000, -32000)),
    });
    let window_b = platform.create_window(WindowOptions {
        title: "slate-golden-trace-two-win-b".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32001, -32001)),
    });

    let req_a = RedrawRequester::new(wake_run_loop);
    let req_b = RedrawRequester::new(wake_run_loop);
    let executor = Executor::new(req_a.clone());
    let runtime = slate_reactive::Runtime::new();

    let state = Rc::new(AppState::new(executor, req_a.clone(), runtime.clone()));

    let id_a = window_a.id();
    let id_b = window_b.id();

    {
        let win_a = WindowState::new(window_a, runtime.clone());
        let win_b = WindowState::new(window_b, runtime);
        let mut windows = state.windows.borrow_mut();
        windows.insert(id_a, win_a);
        windows.insert(id_b, win_b);
    }

    state.register_redraw_requester_for_test(id_a, req_a);
    state.register_redraw_requester_for_test(id_b, req_b);

    let _ = platform;
    (state, id_a, id_b)
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

fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds {
    Bounds {
        origin: Point::new(x, y),
        size: Size::new(w, h),
    }
}

// ---------------------------------------------------------------------------
// Trace recorder (mirrors dispatch_golden_trace.rs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct TraceEntry {
    seq: u32,
    /// Which window this entry belongs to. Lets reviewers confirm routing.
    window: &'static str,
    kind: &'static str,
    handler_on: Option<u64>,
    ctx_target: Option<u64>,
    signal: Option<&'static str>,
    note: Option<&'static str>,
}

#[derive(Clone)]
struct Recorder(Arc<Mutex<Trace>>);

struct Trace {
    seq: u32,
    entries: Vec<TraceEntry>,
}

impl Recorder {
    fn new() -> Self {
        Recorder(Arc::new(Mutex::new(Trace {
            seq: 0,
            entries: Vec::new(),
        })))
    }

    fn push(
        &self,
        window: &'static str,
        kind: &'static str,
        handler_on: Option<ElementId>,
        ctx_target: Option<ElementId>,
        signal: Option<&'static str>,
        note: Option<&'static str>,
    ) {
        let mut t = self.0.lock().unwrap();
        let seq = t.seq;
        t.seq += 1;
        t.entries.push(TraceEntry {
            seq,
            window,
            kind,
            handler_on: handler_on.map(|e| e.as_u64()),
            ctx_target: ctx_target.map(|e| e.as_u64()),
            signal,
            note,
        });
    }

    fn snapshot(&self) -> Vec<TraceEntry> {
        self.0.lock().unwrap().entries.clone()
    }
}

fn signal_label(s: AppSignal) -> &'static str {
    match s {
        AppSignal::None => "None",
        AppSignal::RequestRedraw { .. } => "RequestRedraw",
        AppSignal::RequestQuit => "RequestQuit",
    }
}

// ---------------------------------------------------------------------------
// Handler wiring helpers
// ---------------------------------------------------------------------------

type ArcMouse = Arc<dyn Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static>;
type ArcKey = Arc<dyn Fn(&KeyEvent, &mut EventCtx) + Send + Sync + 'static>;

fn wire_mouse_down(
    state: &AppState,
    win: WindowId,
    win_label: &'static str,
    elem: ElementId,
    b: Bounds,
    rec: &Recorder,
    label: &'static str,
) {
    let r = rec.clone();
    let on_down: ArcMouse = Arc::new(move |_ev, cx| {
        r.push(win_label, label, Some(elem), cx.element_id(), None, None);
    });
    state.install_element_mouse_handlers_for_test(
        win,
        elem,
        MouseHandlers {
            on_mouse_down: Some(on_down),
            on_mouse_move: None,
            on_mouse_up: None,
        },
    );
    state.push_hit_region_for_test(win, HitRegion::new(elem, b, 0));
}

fn wire_key_down(
    state: &AppState,
    win: WindowId,
    win_label: &'static str,
    elem: ElementId,
    rec: &Recorder,
) {
    let r = rec.clone();
    let on_down: ArcKey = Arc::new(move |ev, cx| {
        let note = match &ev.key {
            Key::Named(NamedKey::Tab) => Some("Tab"),
            _ => None,
        };
        r.push(
            win_label,
            "element_key_down",
            Some(elem),
            cx.element_id(),
            None,
            note,
        );
    });
    state.install_element_key_handlers_for_test(
        win,
        elem,
        KeyHandlers {
            on_key_down: Some(on_down),
            on_key_up: None,
            on_text_input: None,
        },
    );
}

// ---------------------------------------------------------------------------
// Scene TW-A: WinA mouse-down → WinB key-down → WinA mouse-up
//
// Interleaves mouse events on window A with a key event on window B. Verifies
// that the trace correctly attributes each entry to its originating window and
// that capture state on window A does not interfere with the key dispatch on B.
// ---------------------------------------------------------------------------

fn scene_twa_interleaved_mouse_a_key_b() -> Vec<TraceEntry> {
    let (state, win_a, win_b) = make_two_window_state();
    let rec = Recorder::new();

    let row_a = id(1);
    let elem_b = id(2);

    // Window A: single clickable row.
    wire_mouse_down(
        &state,
        win_a,
        "winA",
        row_a,
        bounds(0.0, 0.0, 100.0, 50.0),
        &rec,
        "rowA_down",
    );

    // Window B: focusable element with key handler.
    state.register_focusable_for_test(win_b, entry(2));
    state.set_focus_for_test(win_b, elem_b);
    wire_key_down(&state, win_b, "winB", elem_b, &rec);

    let mods = Modifiers::default();

    // 1. WinA mouse-down
    rec.push(
        "winA",
        "BeginDispatch",
        None,
        None,
        None,
        Some("mouse_down:rowA"),
    );
    let s = state.dispatch_mouse_down_for_test(win_a, (10.0, 10.0), MouseButton::Left, mods);
    rec.push(
        "winA",
        "EndDispatch",
        None,
        None,
        Some(signal_label(s)),
        None,
    );

    // 2. WinB key-down (Tab)
    rec.push(
        "winB",
        "BeginDispatch",
        None,
        None,
        None,
        Some("key:Tab on winB"),
    );
    let s = state.dispatch_key_down_for_test(
        win_b,
        KeyCode::Tab,
        Key::Named(NamedKey::Tab),
        mods,
        false,
    );
    rec.push(
        "winB",
        "EndDispatch",
        None,
        None,
        Some(signal_label(s)),
        None,
    );

    // 3. WinA mouse-up
    rec.push(
        "winA",
        "BeginDispatch",
        None,
        None,
        None,
        Some("mouse_up:rowA"),
    );
    let s = state.dispatch_mouse_up_for_test(win_a, (10.0, 10.0), MouseButton::Left, mods);
    rec.push(
        "winA",
        "EndDispatch",
        None,
        None,
        Some(signal_label(s)),
        None,
    );

    rec.snapshot()
}

// ---------------------------------------------------------------------------
// Scene TW-B: Independent focus on both windows + text input
//
// Both windows have a focused element. Text input dispatched to each window
// should route to that window's focused chain only.
// ---------------------------------------------------------------------------

fn scene_twb_independent_focus_text_input() -> Vec<TraceEntry> {
    let (state, win_a, win_b) = make_two_window_state();
    let rec = Recorder::new();

    let field_a = id(1);
    let field_b = id(2);

    state.register_focusable_for_test(win_a, entry(1));
    state.set_focus_for_test(win_a, field_a);

    state.register_focusable_for_test(win_b, entry(2));
    state.set_focus_for_test(win_b, field_b);

    // Wire mouse + key handlers to confirm correct routing.
    wire_key_down(&state, win_a, "winA", field_a, &rec);
    wire_key_down(&state, win_b, "winB", field_b, &rec);

    let mods = Modifiers::default();

    // Text input to win A.
    rec.push(
        "winA",
        "BeginDispatch",
        None,
        None,
        None,
        Some("text_input:x on winA"),
    );
    let s = state.dispatch_text_input_for_test(win_a, "x".into());
    rec.push(
        "winA",
        "EndDispatch",
        None,
        None,
        Some(signal_label(s)),
        None,
    );

    // Text input to win B.
    rec.push(
        "winB",
        "BeginDispatch",
        None,
        None,
        None,
        Some("text_input:y on winB"),
    );
    let s = state.dispatch_text_input_for_test(win_b, "y".into());
    rec.push(
        "winB",
        "EndDispatch",
        None,
        None,
        Some(signal_label(s)),
        None,
    );

    // Key-down to win A (element key fires on A only).
    rec.push(
        "winA",
        "BeginDispatch",
        None,
        None,
        None,
        Some("key:Tab on winA"),
    );
    let s = state.dispatch_key_down_for_test(
        win_a,
        KeyCode::Tab,
        Key::Named(NamedKey::Tab),
        mods,
        false,
    );
    rec.push(
        "winA",
        "EndDispatch",
        None,
        None,
        Some(signal_label(s)),
        None,
    );

    rec.snapshot()
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

fn check_scene_twa() -> Result<(), String> {
    let trace = scene_twa_interleaved_mouse_a_key_b();
    assert_debug_snapshot!("scene_twa_interleaved_mouse_a_key_b", trace);
    Ok(())
}

fn check_scene_twb() -> Result<(), String> {
    let trace = scene_twb_independent_focus_text_input();
    assert_debug_snapshot!("scene_twb_independent_focus_text_input", trace);
    Ok(())
}

fn main() {
    let cases: &[Case] = &[
        ("scene_twa_interleaved_mouse_a_key_b", check_scene_twa),
        ("scene_twb_independent_focus_text_input", check_scene_twb),
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
