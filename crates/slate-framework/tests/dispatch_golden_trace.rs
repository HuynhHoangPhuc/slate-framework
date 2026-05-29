//! Golden-trace snapshot test that pins the ordered handler-invocation trace
//! produced by `AppState::dispatch_*` for a fixed set of fixture scenes.
//!
//! Why this exists: subsequent re-structure phases split the dispatch monolith
//! (`app_state.rs`, `mac/view.rs`, `win/message_loop.rs`). A silent re-ordering
//! of handler invocation, focus application, or capture-routing during those
//! splits would only surface as runtime UX regressions. This test makes any
//! such reordering a snapshot diff that has to be reviewed and explicitly
//! blessed via `cargo insta review`.
//!
//! Recorder strategy: external. Each fixture installs instrumented handlers
//! that append to a shared `Vec<TraceEntry>`; the test bookends every
//! `dispatch_*_for_test` call with manual `BeginDispatch` / `EndDispatch`
//! entries. No changes to `AppState` surface — the trace observes the
//! framework through its existing public dispatch API.
//!
//! Runs cross-platform as a `harness = false` `[[test]]` so its `fn main`
//! executes on the process main thread (AppKit requires it for
//! `MainThreadMarker` on macOS); on Windows the same pattern is fine.
//! `required-features = ["test-hooks"]` gates the build so the `*_for_test`
//! hooks are always present.
//!
//! Bless workflow: see `docs/code-standards.md` → "Snapshot test blessing".

#![cfg(all(
    any(target_os = "macos", target_os = "windows"),
    feature = "test-hooks"
))]

use std::rc::Rc;
use std::sync::{Arc, Mutex};

use insta::assert_debug_snapshot;
use slate_framework::app_state::window_state::WindowState;
use slate_framework::app_state::{AppSignal, AppState};
use slate_framework::event::{
    EventCtx, ImeCommitEvent, ImeHandlers, ImePreeditEvent, KeyEvent, KeyHandlers, MouseEvent,
    MouseHandlers, TextInputEvent,
};
use slate_framework::executor::{Executor, RedrawRequester};
use slate_framework::focus::FocusableEntry;
use slate_framework::hit_test::HitRegion;
use slate_framework::types::{Bounds, ElementId, Point, Size};
use slate_framework::{Key, KeyCode, Modifiers, MouseButton, NamedKey};
use slate_platform::{DefaultPlatform, Platform, Window, WindowId, WindowOptions, wake_run_loop};

// ---------------------------------------------------------------------------
// Harness scaffolding (mirrors the cross-platform `harness = false` tests)
// ---------------------------------------------------------------------------

type Case = (&'static str, fn() -> Result<(), String>);

/// Construct an `AppState` with one registered test window, returning the
/// state and the `WindowId` of that window.
fn make_state() -> (Rc<AppState>, WindowId) {
    let platform = DefaultPlatform::new();
    let window = platform.create_window(WindowOptions {
        title: "slate-dispatch-golden-trace".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32000, -32000)),
    });
    let redraw_requester = RedrawRequester::new(wake_run_loop);
    let executor = Executor::new(redraw_requester.clone());
    let runtime = slate_reactive::Runtime::new();

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

    // Drop platform last so the window handle stays valid.
    let _ = platform;

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

fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds {
    Bounds {
        origin: Point::new(x, y),
        size: Size::new(w, h),
    }
}

// ---------------------------------------------------------------------------
// Trace recorder
// ---------------------------------------------------------------------------

/// One observable step. Stable across rebuilds: no addresses, no timing,
/// no platform-dependent strings. Element ids are surfaced as their raw u64
/// (also stable because the fixtures call `id(n)` with hardcoded `n`).
///
/// `dead_code` is silenced because every field is read via the derived
/// `Debug` impl into the `insta` snapshot — the compiler can't see that
/// path and would otherwise trip the workspace `-D warnings` gate.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct TraceEntry {
    seq: u32,
    kind: &'static str,
    /// Element this handler is bound to (closure capture). `None` for
    /// app-level handlers and for `Begin/End` bookends.
    handler_on: Option<u64>,
    /// `EventCtx::element_id()` at the point the handler fires. Diverges
    /// from `handler_on` if dispatch ever re-routes through a different
    /// element — that divergence IS the signal we want to catch.
    ctx_target: Option<u64>,
    /// `Begin/End` bookends record the `AppSignal` returned by the
    /// dispatch method; individual handler entries leave it `None`.
    signal: Option<&'static str>,
    /// Free-form note: focus target for focus-driven scenes, captured
    /// element id for capture-driven scenes, etc.
    note: Option<&'static str>,
}

/// Send + Sync because element-bound handlers (`MouseHandler`,
/// `ElementKeyHandler`, `ElementImePreeditHandler`, …) are typed
/// `Arc<dyn Fn + Send + Sync>`. App-level handlers are `Box<dyn FnMut>` and
/// happily hold this too.
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
            kind,
            handler_on: handler_on.map(elem_raw),
            ctx_target: ctx_target.map(elem_raw),
            signal,
            note,
        });
    }

    /// Snapshot the trace so far without requiring unique ownership of the
    /// inner Arc. Callers usually invoke this after all dispatches have run
    /// but while the `AppState` (which owns the installed handlers, each
    /// holding a recorder clone) is still alive.
    fn snapshot(&self) -> Vec<TraceEntry> {
        self.0.lock().unwrap().entries.clone()
    }
}

/// Raw u64 view of an ElementId. The fixtures construct ids via
/// `ElementId::from_raw(n)`, and `as_u64()` is the public inverse — using it
/// keeps the snapshot stable across any future change to ElementId's `Debug`
/// formatting.
fn elem_raw(e: ElementId) -> u64 {
    e.as_u64()
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

/// Element-bound handler bundles are typed `Arc<dyn Fn + Send + Sync>` by
/// the framework. Boxed handler bundles for app-level wiring are kept under
/// `Box<dyn Fn>` for symmetry with `install_key_handlers_for_test`.
type ArcMouse = Arc<dyn Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static>;
type ArcKey = Arc<dyn Fn(&KeyEvent, &mut EventCtx) + Send + Sync + 'static>;
type ArcImePreedit = Arc<dyn Fn(&ImePreeditEvent, &mut EventCtx) + Send + Sync + 'static>;
type ArcImeCommit = Arc<dyn Fn(&ImeCommitEvent, &mut EventCtx) + Send + Sync + 'static>;
type BoxKey = Box<dyn FnMut(&KeyEvent, &mut EventCtx)>;
type BoxText = Box<dyn FnMut(&TextInputEvent, &mut EventCtx)>;

/// Install an instrumented mouse-down handler on `elem` that records its
/// invocation, then install a matching hit region.
fn wire_element_mouse_down(
    state: &AppState,
    window: WindowId,
    elem: ElementId,
    b: Bounds,
    rec: &Recorder,
    label: &'static str,
) {
    let r = rec.clone();
    let on_down: ArcMouse = Arc::new(move |_ev, cx| {
        r.push(label, Some(elem), cx.element_id(), None, None);
    });
    state.install_element_mouse_handlers_for_test(
        window,
        elem,
        MouseHandlers {
            on_mouse_down: Some(on_down),
            on_mouse_move: None,
            on_mouse_up: None,
        },
    );
    state.push_hit_region_for_test(window, HitRegion::new(elem, b, 0));
}

/// Install an instrumented mouse-down handler that ALSO grabs explicit capture
/// of `capture_to`. Used in scene C to verify capture-routing trace ordering.
fn wire_element_mouse_down_with_capture(
    state: &AppState,
    window: WindowId,
    elem: ElementId,
    b: Bounds,
    capture_to: ElementId,
    rec: &Recorder,
    label: &'static str,
) {
    let r = rec.clone();
    let on_down: ArcMouse = Arc::new(move |_ev, cx| {
        r.push(
            label,
            Some(elem),
            cx.element_id(),
            None,
            Some("set_capture"),
        );
        cx.set_capture(capture_to);
    });
    let r2 = rec.clone();
    let on_move: ArcMouse = Arc::new(move |_ev, cx| {
        r2.push(
            "element_mouse_move",
            Some(elem),
            cx.element_id(),
            None,
            None,
        );
    });
    state.install_element_mouse_handlers_for_test(
        window,
        elem,
        MouseHandlers {
            on_mouse_down: Some(on_down),
            on_mouse_move: Some(on_move),
            on_mouse_up: None,
        },
    );
    state.push_hit_region_for_test(window, HitRegion::new(elem, b, 0));
}

/// Install instrumented per-element IME handlers (preedit + commit).
fn wire_element_ime(state: &AppState, window: WindowId, elem: ElementId, rec: &Recorder) {
    let rp = rec.clone();
    let on_preedit: ArcImePreedit = Arc::new(move |_e, cx| {
        rp.push(
            "element_ime_preedit",
            Some(elem),
            cx.element_id(),
            None,
            None,
        );
    });
    let rc = rec.clone();
    let on_commit: ArcImeCommit = Arc::new(move |_e, cx| {
        rc.push(
            "element_ime_commit",
            Some(elem),
            cx.element_id(),
            None,
            None,
        );
    });
    state.install_element_ime_handlers_for_test(
        window,
        elem,
        ImeHandlers {
            on_ime_preedit: Some(on_preedit),
            on_ime_commit: Some(on_commit),
            ..Default::default()
        },
    );
}

/// Install instrumented app-level key + text-input handlers.
fn wire_app_keys(state: &AppState, rec: &Recorder) {
    let rd = rec.clone();
    let on_down: BoxKey = Box::new(move |ev, cx| {
        let note = match &ev.key {
            Key::Named(NamedKey::Tab) => Some("Tab"),
            Key::Named(NamedKey::Escape) => Some("Escape"),
            _ => None,
        };
        rd.push("app_key_down", None, cx.element_id(), None, note);
    });
    let ru = rec.clone();
    let on_up: BoxKey = Box::new(move |_ev, cx| {
        ru.push("app_key_up", None, cx.element_id(), None, None);
    });
    let rt = rec.clone();
    let on_text: BoxText = Box::new(move |_ev, cx| {
        rt.push("app_text_input", None, cx.element_id(), None, None);
    });
    state.install_key_handlers_for_test(vec![on_down], vec![on_up], vec![on_text]);
}

/// Install instrumented per-element key handlers (down only — enough for the
/// ordering pin).
fn wire_element_keys(state: &AppState, window: WindowId, elem: ElementId, rec: &Recorder) {
    let r = rec.clone();
    let on_down: ArcKey = Arc::new(move |ev, cx| {
        let note = match &ev.key {
            Key::Named(NamedKey::Tab) => Some("Tab"),
            Key::Named(NamedKey::Escape) => Some("Escape"),
            _ => None,
        };
        r.push("element_key_down", Some(elem), cx.element_id(), None, note);
    });
    state.install_element_key_handlers_for_test(
        window,
        elem,
        KeyHandlers {
            on_key_down: Some(on_down),
            on_key_up: None,
            on_text_input: None,
        },
    );
}

// ---------------------------------------------------------------------------
// Scene A: focused single element receives "abc" → Tab → Esc
// ---------------------------------------------------------------------------

fn scene_a_text_input_then_tab_then_esc() -> Vec<TraceEntry> {
    let (state, win) = make_state();
    let rec = Recorder::new();

    let field = id(1);
    state.register_focusable_for_test(win, entry(1));
    state.set_focus_for_test(win, field);
    wire_app_keys(&state, &rec);
    wire_element_keys(&state, win, field, &rec);

    let mods = Modifiers::default();

    // 1) "abc" → one TextInput per char (matches platform char-event cadence)
    for ch in ['a', 'b', 'c'] {
        rec.push("BeginDispatch", None, None, None, Some("text_input:char"));
        let s = state.dispatch_text_input_for_test(win, ch.to_string());
        rec.push("EndDispatch", None, None, Some(signal_label(s)), None);
    }

    // 2) Tab down/up (focus traversal is owned by Tab handler in apps;
    //    here we only pin that the down/up routes are invoked).
    rec.push("BeginDispatch", None, None, None, Some("key:Tab"));
    let s =
        state.dispatch_key_down_for_test(win, KeyCode::Tab, Key::Named(NamedKey::Tab), mods, false);
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);
    rec.push("BeginDispatch", None, None, None, Some("key:Tab_up"));
    let s = state.dispatch_key_up_for_test(win, KeyCode::Tab, Key::Named(NamedKey::Tab), mods);
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);

    // 3) Escape down (mirrors the textfield "cancel composition" path)
    rec.push("BeginDispatch", None, None, None, Some("key:Escape"));
    let s = state.dispatch_key_down_for_test(
        win,
        KeyCode::Escape,
        Key::Named(NamedKey::Escape),
        mods,
        false,
    );
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);

    rec.snapshot()
}

// ---------------------------------------------------------------------------
// Scene B: two stacked elements — click row 1, click row 2 (focus shift),
//          IME preedit + commit on the focused element
// ---------------------------------------------------------------------------

fn scene_b_click_focus_shift_then_ime() -> Vec<TraceEntry> {
    let (state, win) = make_state();
    let rec = Recorder::new();

    let row1 = id(1);
    let row2 = id(2);
    state.register_focusable_for_test(win, entry(1));
    state.register_focusable_for_test(win, entry(2));

    wire_element_mouse_down(
        &state,
        win,
        row1,
        bounds(0.0, 0.0, 100.0, 50.0),
        &rec,
        "row1_down",
    );
    wire_element_mouse_down(
        &state,
        win,
        row2,
        bounds(0.0, 50.0, 100.0, 50.0),
        &rec,
        "row2_down",
    );
    wire_element_ime(&state, win, row1, &rec);
    wire_element_ime(&state, win, row2, &rec);

    let mods = Modifiers::default();

    rec.push("BeginDispatch", None, None, None, Some("mouse_down:row1"));
    let s = state.dispatch_mouse_down_for_test(win, (10.0, 10.0), MouseButton::Left, mods);
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);
    // Release the implicit mouse-down capture (framework auto-captures the
    // hit target on every mouse-down and only releases on mouse-up). Without
    // this, the second click would dispatch to row1 regardless of position.
    rec.push("BeginDispatch", None, None, None, Some("mouse_up:row1"));
    let s = state.dispatch_mouse_up_for_test(win, (10.0, 10.0), MouseButton::Left, mods);
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);

    // mouse_down on row2 auto-focuses row2 (focusable-ancestor lookup in
    // dispatch_mouse_down). Subsequent IME thus routes to row2 without any
    // explicit set_focus call here.
    rec.push("BeginDispatch", None, None, None, Some("mouse_down:row2"));
    let s = state.dispatch_mouse_down_for_test(win, (10.0, 60.0), MouseButton::Left, mods);
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);
    rec.push("BeginDispatch", None, None, None, Some("mouse_up:row2"));
    let s = state.dispatch_mouse_up_for_test(win, (10.0, 60.0), MouseButton::Left, mods);
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);

    rec.push(
        "BeginDispatch",
        None,
        None,
        None,
        Some("ime_preedit:に on row2"),
    );
    let s = state.dispatch_ime_preedit_for_test(win, "に".into(), 1, None);
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);

    rec.push(
        "BeginDispatch",
        None,
        None,
        None,
        Some("ime_commit:日 on row2"),
    );
    let s = state.dispatch_ime_commit_for_test(win, "日".into());
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);

    rec.snapshot()
}

// ---------------------------------------------------------------------------
// Scene C: mouse-capture drag from row 1 across hit-test boundary into row 2;
//          capture target stays row 1 across the boundary cross.
// ---------------------------------------------------------------------------

fn scene_c_capture_drag_across_boundary() -> Vec<TraceEntry> {
    let (state, win) = make_state();
    let rec = Recorder::new();

    let row1 = id(1);
    let row2 = id(2);
    wire_element_mouse_down_with_capture(
        &state,
        win,
        row1,
        bounds(0.0, 0.0, 100.0, 50.0),
        row1,
        &rec,
        "row1_down",
    );
    wire_element_mouse_down(
        &state,
        win,
        row2,
        bounds(0.0, 50.0, 100.0, 50.0),
        &rec,
        "row2_down",
    );

    let mods = Modifiers::default();

    rec.push(
        "BeginDispatch",
        None,
        None,
        None,
        Some("mouse_down:row1 (sets capture)"),
    );
    let s = state.dispatch_mouse_down_for_test(win, (10.0, 10.0), MouseButton::Left, mods);
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);
    let captured = state.capture_target_for_test(win);
    let captured_note: &'static str = match captured {
        Some(e) if e == row1 => "capture=row1",
        Some(_) => "capture=other",
        None => "capture=none",
    };
    rec.push("Observe", None, None, None, Some(captured_note));

    // Move into row2's hit region. With explicit capture on row1, mouse
    // routing should stay with row1 — the trace will reveal if a future
    // refactor accidentally re-targets to row2.
    rec.push(
        "BeginDispatch",
        None,
        None,
        None,
        Some("mouse_move:into row2 region"),
    );
    let s = state.dispatch_mouse_move_for_test(win, (10.0, 60.0));
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);

    rec.push(
        "BeginDispatch",
        None,
        None,
        None,
        Some("mouse_up:in row2 region"),
    );
    let s = state.dispatch_mouse_up_for_test(win, (10.0, 60.0), MouseButton::Left, mods);
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);
    let captured_after = state.capture_target_for_test(win);
    let after_note: &'static str = match captured_after {
        Some(e) if e == row1 => "capture=row1(sticky)",
        Some(_) => "capture=other",
        None => "capture=none",
    };
    rec.push("Observe", None, None, None, Some(after_note));

    rec.snapshot()
}

// ---------------------------------------------------------------------------
// Scene D: IME commit while a focused element owns IME; followed by Tab.
//          Mirrors the "Tab-commit" semantics covered by `260522-2309` work.
// ---------------------------------------------------------------------------

fn scene_d_ime_commit_then_tab() -> Vec<TraceEntry> {
    let (state, win) = make_state();
    let rec = Recorder::new();

    let owner = id(1);
    state.register_focusable_for_test(win, entry(1));
    state.set_focus_for_test(win, owner);
    wire_element_ime(&state, win, owner, &rec);
    wire_app_keys(&state, &rec);
    wire_element_keys(&state, win, owner, &rec);

    let mods = Modifiers::default();

    rec.push("BeginDispatch", None, None, None, Some("ime_preedit:あ"));
    let s = state.dispatch_ime_preedit_for_test(win, "あ".into(), 1, None);
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);

    rec.push(
        "BeginDispatch",
        None,
        None,
        None,
        Some("ime_commit:愛 (focus owner active)"),
    );
    let s = state.dispatch_ime_commit_for_test(win, "愛".into());
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);

    rec.push(
        "BeginDispatch",
        None,
        None,
        None,
        Some("key:Tab (post-commit)"),
    );
    let s =
        state.dispatch_key_down_for_test(win, KeyCode::Tab, Key::Named(NamedKey::Tab), mods, false);
    rec.push("EndDispatch", None, None, Some(signal_label(s)), None);

    rec.snapshot()
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

fn check_scene_a() -> Result<(), String> {
    let trace = scene_a_text_input_then_tab_then_esc();
    assert_debug_snapshot!("scene_a_text_input_tab_esc", trace);
    Ok(())
}

fn check_scene_b() -> Result<(), String> {
    let trace = scene_b_click_focus_shift_then_ime();
    assert_debug_snapshot!("scene_b_click_focus_shift_ime", trace);
    Ok(())
}

fn check_scene_c() -> Result<(), String> {
    let trace = scene_c_capture_drag_across_boundary();
    assert_debug_snapshot!("scene_c_capture_drag_across_boundary", trace);
    Ok(())
}

fn check_scene_d() -> Result<(), String> {
    let trace = scene_d_ime_commit_then_tab();
    assert_debug_snapshot!("scene_d_ime_commit_then_tab", trace);
    Ok(())
}

fn main() {
    let cases: &[Case] = &[
        ("scene_a_text_input_tab_esc", check_scene_a),
        ("scene_b_click_focus_shift_ime", check_scene_b),
        ("scene_c_capture_drag_across_boundary", check_scene_c),
        ("scene_d_ime_commit_then_tab", check_scene_d),
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
    println!("\nall {} case(s) cases passed", cases.len());
}
