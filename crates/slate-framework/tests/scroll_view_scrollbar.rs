//! ScrollView scrollbar-thumb regression tests.
//!
//! Proves the thumb is painted only when content overflows, sits at the
//! viewport's right edge, tracks the scroll offset, and resolves hover / active
//! fills through the same interaction snapshot that powers `Div` styling.
//! The pure thumb geometry + drag math live in `scroll_view::scrollbar` unit
//! tests; these exercise the wired element via the headless paint pipeline.

use slate_framework::reactive::Signal;
use slate_framework::{AnyElement, Color, Div, HeadlessApp, IntoAny, ScrollView, View};

const VIEWPORT_W: f32 = 120.0;
const VIEWPORT_H: f32 = 100.0;
const ROW_H: f32 = 60.0;

/// Three 60px rows (180px) inside a 100px viewport → 80px of scroll.
struct ScrollDemo {
    offset: Signal<f32>,
}

impl View for ScrollDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        ScrollView::new()
            .offset(self.offset.clone())
            .style(|s| s.width(VIEWPORT_W).height(VIEWPORT_H).column())
            .child(row(Color::RED))
            .child(row(Color::GREEN))
            .child(row(Color::BLUE))
            .into_any()
    }
}

fn row(color: Color) -> Div {
    Div::new()
        .background(color)
        .style(|s| s.width(VIEWPORT_W).height(ROW_H))
}

/// The thumb is the last rect pushed (after the content rows).
fn thumb_rect(app: &HeadlessApp) -> slate_renderer::scene::RectInstance {
    *app.scene().rects.last().expect("at least one rect")
}

#[test]
fn thumb_painted_at_right_edge_when_scrollable() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, 200, 1.0).expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    let mut view = ScrollDemo { offset };
    let _ = app.render_view(&mut view).expect("render");

    // 3 row rects + 1 thumb rect.
    assert_eq!(app.scene().rects.len(), 4, "thumb adds exactly one rect");
    let thumb = thumb_rect(&app);
    // Right-aligned: x = width - SCROLLBAR_WIDTH(8) - MARGIN(2) = 110.
    assert_eq!(thumb.rect[0], slate_renderer::Lpx(110.0));
    // Thumb length = track(96) * viewport(100)/content(180) ≈ 53.33.
    let len = thumb.rect[3].0;
    assert!((len - 96.0 * 100.0 / 180.0).abs() < 0.5, "thumb len = {len}");
}

#[test]
fn no_thumb_when_content_fits() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, 200, 1.0).expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    // One short row (60px) inside a 100px viewport → nothing to scroll.
    struct FitsView {
        offset: Signal<f32>,
    }
    impl View for FitsView {
        fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
            ScrollView::new()
                .offset(self.offset.clone())
                .style(|s| s.width(VIEWPORT_W).height(VIEWPORT_H).column())
                .child(row(Color::RED))
                .into_any()
        }
    }
    let mut view = FitsView { offset };
    let _ = app.render_view(&mut view).expect("render");
    assert_eq!(app.scene().rects.len(), 1, "no thumb when content fits");
}

#[test]
fn thumb_tracks_offset_downward() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, 200, 1.0).expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    let mut view = ScrollDemo {
        offset: offset.clone(),
    };

    let _ = app.render_view(&mut view).expect("first render");
    let top_y = thumb_rect(&app).rect[1].0;

    offset.set(80.0); // fully scrolled
    let _ = app.render_view(&mut view).expect("second render");
    let bottom_y = thumb_rect(&app).rect[1].0;

    assert!(bottom_y > top_y, "thumb moves down as offset grows");
}

#[test]
fn thumb_fill_reacts_to_hover_and_press() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, 200, 1.0).expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    let mut view = ScrollDemo { offset };

    // Resting fill (no cursor).
    let _ = app.render_view(&mut view).expect("render");
    let resting = thumb_rect(&app).color;

    // Cursor over the thumb (right edge, near the top) → hover fill.
    app.set_cursor(Some((114.0, 10.0)));
    let _ = app.render_view(&mut view).expect("render");
    let hovered = thumb_rect(&app).color;
    assert_ne!(resting, hovered, "hover changes the thumb fill");

    // Pressing on top of hover → active fill (distinct from hover).
    app.set_pressing(true);
    let _ = app.render_view(&mut view).expect("render");
    let pressed = thumb_rect(&app).color;
    assert_ne!(hovered, pressed, "press changes the thumb fill further");
}
