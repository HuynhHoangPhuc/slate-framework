//! ScrollView clip + scroll-translate regression tests.
//!
//! Proves the renderer clip bridge (step 3 of the scroll phase):
//! - a ScrollView emits a layer scissored to its viewport bounds, and
//! - children paint translated up by the caller-owned offset signal, which
//!   re-resolves under the whole-view rebuild after `Signal::set`.

use slate_framework::reactive::Signal;
use slate_framework::{AnyElement, Color, Div, HeadlessApp, IntoAny, ScrollView, View};
use slate_renderer::Lpx;
use slate_renderer::scene::ClipRect;

const VIEWPORT_H: f32 = 100.0;
const VIEWPORT_W: f32 = 120.0;
const ROW_H: f32 = 60.0;

/// Three stacked rows (180px content) inside a 100px viewport → 80px of scroll.
fn scroll_view(offset: Signal<f32>) -> AnyElement {
    ScrollView::new()
        .offset(offset)
        .style(|s| s.width(VIEWPORT_W).height(VIEWPORT_H).column())
        .child(row(Color::RED))
        .child(row(Color::GREEN))
        .child(row(Color::BLUE))
        .into_any()
}

fn row(color: Color) -> Div {
    Div::new()
        .background(color)
        .style(|s| s.width(VIEWPORT_W).height(ROW_H))
}

struct ScrollDemo {
    offset: Signal<f32>,
}

impl View for ScrollDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        scroll_view(self.offset.clone())
    }
}

#[test]
fn scrollview_emits_layer_clipped_to_viewport() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, 200, 1.0).expect("app");
    let runtime = app.runtime();
    let _ = app
        .render(scroll_view(Signal::new(runtime, 0.0)))
        .expect("render");

    // Exactly one layer carries the viewport clip rect.
    let clipped: Vec<ClipRect> = app
        .scene()
        .layers
        .iter()
        .filter_map(|l| l.clip)
        .collect();
    assert_eq!(clipped.len(), 1, "expected one clipped layer");
    assert_eq!(
        clipped[0],
        ClipRect::new(Lpx(0.0), Lpx(0.0), Lpx(VIEWPORT_W), Lpx(VIEWPORT_H)),
    );
}

#[test]
fn children_translate_up_by_offset_after_set() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, 200, 1.0).expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    let mut view = ScrollDemo {
        offset: offset.clone(),
    };

    // At offset 0 the first row sits at the top of the viewport.
    let _ = app.render_view(&mut view).expect("first render");
    assert_eq!(app.scene().rects[0].rect[1], Lpx(0.0));
    assert_eq!(app.scene().rects[1].rect[1], Lpx(ROW_H)); // second row below it

    // Scroll down 50px → every row shifts up by 50.
    offset.set(50.0);
    let _ = app.render_view(&mut view).expect("second render");
    assert_eq!(app.scene().rects[0].rect[1], Lpx(-50.0));
    assert_eq!(app.scene().rects[1].rect[1], Lpx(ROW_H - 50.0));
}

#[test]
fn offset_clamps_to_scrollable_range() {
    // Content 180, viewport 100 → max scroll 80. A larger set value clamps.
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, 200, 1.0).expect("app");
    let offset = Signal::new(app.runtime(), 999.0);
    let mut view = ScrollDemo {
        offset: offset.clone(),
    };
    let _ = app.render_view(&mut view).expect("render");
    // First row top clamped to -80 (not -999).
    assert_eq!(app.scene().rects[0].rect[1], Lpx(-80.0));
}

#[test]
fn max_offset_includes_bottom_padding() {
    // Viewport 100 with 20px bottom padding; content 180 → scrollable range is
    // (180 + 20) - 100 = 100, so the first row can reach -100 (revealing the
    // trailing padding), not -80.
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, 220, 1.0).expect("app");
    let offset = Signal::new(app.runtime(), 999.0);
    let mut view = PaddedScrollDemo {
        offset: offset.clone(),
    };
    let _ = app.render_view(&mut view).expect("render");
    assert_eq!(app.scene().rects[0].rect[1], Lpx(-100.0));
}

struct PaddedScrollDemo {
    offset: Signal<f32>,
}

impl View for PaddedScrollDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        ScrollView::new()
            .offset(self.offset.clone())
            .style(|s| {
                s.width(VIEWPORT_W)
                    .height(VIEWPORT_H)
                    .column()
                    .padding_all(20.0)
            })
            .child(row(Color::RED))
            .child(row(Color::GREEN))
            .child(row(Color::BLUE))
            .into_any()
    }
}

#[test]
fn clip_only_scrollview_without_offset_still_clips() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, 200, 1.0).expect("app");
    let _ = app
        .render(
            ScrollView::new()
                .style(|s| s.width(VIEWPORT_W).height(VIEWPORT_H).column())
                .child(row(Color::RED))
                .child(row(Color::GREEN))
                .child(row(Color::BLUE))
                .into_any(),
        )
        .expect("render");
    let clipped = app.scene().layers.iter().filter(|l| l.clip.is_some()).count();
    assert_eq!(clipped, 1, "clip-only ScrollView must still emit a clip layer");
    // No offset → first row stays at top.
    assert_eq!(app.scene().rects[0].rect[1], Lpx(0.0));
}
