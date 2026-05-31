//! ScrollView coexistence regression tests (step 8 of the scroll phase).
//!
//! The renderer clip, the scroll offset, and every child-registered coordinate
//! (paint rects, hit regions, IME caret rect) all flow through one translation:
//! children are shifted by `-offset` in prepaint *and* paint. These tests prove
//! the three places that translation has to stay consistent:
//!
//! 1. **Hit-test** — a scrolled child's hit region tracks the offset, so a click
//!    lands on the element actually under the cursor.
//! 2. **Nested scroll** — scroll-inside-scroll composes offsets and intersects
//!    clips (inner clip = inner viewport ∩ outer viewport).
//! 3. **IME caret** — a TextField's `firstRectForCharacterRange` rect maps
//!    through an enclosing scroll offset.

use slate_framework::reactive::Signal;
use slate_framework::{
    AnyElement, Color, Div, HeadlessApp, IntoAny, ScrollView, TextField, View,
};
use slate_renderer::Lpx;
use slate_renderer::scene::ClipRect;

// ---------------------------------------------------------------------------
// 1. Scrolled hit-test
// ---------------------------------------------------------------------------

const HT_W: f32 = 120.0;
const HT_VIEWPORT_H: f32 = 100.0;
const HT_ROW_H: f32 = 60.0;

struct HitTestDemo {
    offset: Signal<f32>,
}

impl View for HitTestDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        // Three 60px rows (180px) in a 100px viewport → 80px of scroll.
        ScrollView::new()
            .offset(self.offset.clone())
            .style(|s| s.width(HT_W).height(HT_VIEWPORT_H).column())
            .child(ht_row(Color::RED))
            .child(ht_row(Color::GREEN))
            .child(ht_row(Color::BLUE))
            .into_any()
    }
}

fn ht_row(color: Color) -> Div {
    // A background makes the Div register a hit region at its (translated) bounds.
    Div::new()
        .background(color)
        .style(|s| s.width(HT_W).height(HT_ROW_H))
}

#[test]
fn hit_test_tracks_scroll_offset() {
    let mut app = HeadlessApp::with_scale_factor(HT_W as u32, 200, 1.0).expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    let mut view = HitTestDemo {
        offset: offset.clone(),
    };

    // Probe at x=60 (clear of the right-edge scrollbar thumb).
    let _ = app.render_view(&mut view).expect("render @0");
    let row0 = app.hit_test(60.0, 40.0).expect("row 0 at y=40");
    let row1 = app.hit_test(60.0, 80.0).expect("row 1 at y=80");
    assert_ne!(row0, row1, "rows 0 and 1 are distinct elements");

    // Scroll down 40px: every row's hit region shifts up by 40, so the point
    // (60,40) — previously over row 0 — is now over row 1.
    offset.set(40.0);
    let _ = app.render_view(&mut view).expect("render @40");
    assert_eq!(
        app.hit_test(60.0, 40.0),
        Some(row1),
        "after scrolling 40px, row 1 is under the point that held row 0"
    );
}

// ---------------------------------------------------------------------------
// 2. Nested scroll (offset composition + clip intersection)
// ---------------------------------------------------------------------------

const N_W: f32 = 200.0;
const OUTER_H: f32 = 100.0;
const SPACER_H: f32 = 40.0;
const INNER_H: f32 = 80.0;
const INNER_ROW_H: f32 = 50.0;

struct NestedDemo {
    outer: Signal<f32>,
    inner: Signal<f32>,
}

impl View for NestedDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        // Outer: 40px spacer + an 80px inner ScrollView = 120px content in a
        // 100px viewport → 20px outer scroll.
        // Inner: three 50px rows = 150px content in an 80px viewport → 70px
        // inner scroll. Inner row `i` is `50 + i` wide (identifies it in the scene).
        let inner = ScrollView::new()
            .offset(self.inner.clone())
            .style(|s| s.width(N_W).height(INNER_H).column())
            .child(inner_row(0))
            .child(inner_row(1))
            .child(inner_row(2));

        ScrollView::new()
            .offset(self.outer.clone())
            .style(|s| s.width(N_W).height(OUTER_H).column())
            .child(Div::new().background(Color::WHITE).style(|s| s.width(N_W).height(SPACER_H)))
            .child(inner)
            .into_any()
    }
}

fn inner_row(i: usize) -> Div {
    Div::new()
        .background(Color::RED)
        .style(move |s| s.width(50.0 + i as f32).height(INNER_ROW_H))
}

/// Y of inner row 1 (width 51) in the painted scene.
fn inner_row1_y(app: &HeadlessApp) -> f32 {
    app.scene()
        .rects
        .iter()
        .find(|r| (r.rect[2].0 - 51.0).abs() < 0.5)
        .expect("inner row 1 painted")
        .rect[1]
        .0
}

fn clipped_rects(app: &HeadlessApp) -> Vec<ClipRect> {
    app.scene().layers.iter().filter_map(|l| l.clip).collect()
}

#[test]
fn nested_scroll_composes_offsets_and_clips() {
    let mut app = HeadlessApp::with_scale_factor(N_W as u32, 200, 1.0).expect("app");
    let outer = Signal::new(app.runtime(), 0.0);
    let inner = Signal::new(app.runtime(), 0.0);
    let mut view = NestedDemo {
        outer: outer.clone(),
        inner: inner.clone(),
    };

    // Unscrolled: inner viewport sits at content y=40. Inner row 1 lives at
    // inner-content y=50 → screen y = 40 + 50 = 90.
    let _ = app.render_view(&mut view).expect("render @(0,0)");
    assert!((inner_row1_y(&app) - 90.0).abs() < 0.5, "row1 y@0 = {}", inner_row1_y(&app));

    let clips = clipped_rects(&app);
    // Outer clip = whole outer viewport.
    assert!(
        clips.iter().any(|c| *c == ClipRect::new(Lpx(0.0), Lpx(0.0), Lpx(N_W), Lpx(OUTER_H))),
        "outer viewport clip present"
    );
    // Inner clip = inner viewport (0,40,200,80) ∩ outer (0,0,200,100) = (0,40,200,60).
    assert!(
        clips.iter().any(|c| *c == ClipRect::new(Lpx(0.0), Lpx(40.0), Lpx(N_W), Lpx(60.0))),
        "inner clip is inner∩outer, got {clips:?}"
    );

    // Scroll outer 20 (its max) and inner 30. Inner viewport screen origin moves
    // to y=20; inner row 1 → 20 + (50 - 30) = 40.
    outer.set(20.0);
    inner.set(30.0);
    let _ = app.render_view(&mut view).expect("render @(20,30)");
    assert!(
        (inner_row1_y(&app) - 40.0).abs() < 0.5,
        "row1 y@(20,30) composes both offsets = {}",
        inner_row1_y(&app)
    );
    // Inner clip now = (0,20,200,80) ∩ (0,0,200,100) = (0,20,200,80).
    let clips = clipped_rects(&app);
    assert!(
        clips.iter().any(|c| *c == ClipRect::new(Lpx(0.0), Lpx(20.0), Lpx(N_W), Lpx(80.0))),
        "inner clip follows the scrolled inner viewport, got {clips:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Scrolled IME caret rect
// ---------------------------------------------------------------------------

struct ImeDemo {
    offset: Signal<f32>,
    text: Signal<String>,
}

impl View for ImeDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        // 40px spacer, a TextField, then a tall spacer so the content (256px)
        // overflows the 100px viewport and the column packs from the top. The
        // field sits at content y=40; scrolling moves its caret rect with it.
        ScrollView::new()
            .offset(self.offset.clone())
            .style(|s| s.width(200.0).height(100.0).column())
            .child(Div::new().background(Color::WHITE).style(|s| s.width(200.0).height(40.0)))
            .child(TextField::new(self.text.clone()))
            .child(Div::new().background(Color::GREEN).style(|s| s.width(200.0).height(200.0)))
            .into_any()
    }
}

#[test]
fn ime_caret_rect_maps_through_scroll() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    let text = Signal::new(app.runtime(), "hi".to_string());
    let mut view = ImeDemo {
        offset: offset.clone(),
        text,
    };

    let _ = app.render_view(&mut view).expect("render @0");
    // The TextField is the focusable that isn't the full-viewport ScrollView.
    let (tf_id, _) = app
        .focusables()
        .into_iter()
        .find(|(_, b)| b.size.height < 100.0)
        .expect("TextField focusable present");
    let caret0 = app.ime_caret_rect(tf_id).expect("caret rect @0");
    assert_eq!(caret0.y, 40, "caret at content y=40 when unscrolled");

    // Scroll 30px → the caret rect must follow the field up to y=10.
    offset.set(30.0);
    let _ = app.render_view(&mut view).expect("render @30");
    let caret1 = app.ime_caret_rect(tf_id).expect("caret rect @30");
    assert_eq!(caret1.y, 10, "caret rect maps through the 30px scroll offset");
}
