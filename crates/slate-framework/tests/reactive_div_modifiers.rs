//! Reactive `Div` modifier regression tests.
//!
//! Static callers must render byte-identically to the pre-reactive API, and a
//! signal-backed `background`/`corner_radius` must re-resolve to the signal's
//! current value after `set()` + re-render under the whole-view rebuild model.

use slate_framework::reactive::Signal;
use slate_framework::{AnyElement, Color, Div, HeadlessApp, IntoAny, View};

#[test]
fn static_background_resolves_to_color() {
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp creation");
    let _ = app
        .render(
            Div::new()
                .background(Color::RED)
                .style(|s| s.width(40.0).height(40.0))
                .into_any(),
        )
        .expect("render");
    assert_eq!(app.scene().rects[0].color, [1.0, 0.0, 0.0, 1.0]);
}

#[test]
fn static_array_background_unchanged() {
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp creation");
    let _ = app
        .render(
            Div::new()
                .background([0.0, 1.0, 0.0, 1.0])
                .style(|s| s.width(40.0).height(40.0))
                .into_any(),
        )
        .expect("render");
    assert_eq!(app.scene().rects[0].color, [0.0, 1.0, 0.0, 1.0]);
}

struct BgView {
    color: Signal<Color>,
}

impl View for BgView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new()
            .background(self.color.clone())
            .style(|s| s.width(40.0).height(40.0))
            .into_any()
    }
}

#[test]
fn signal_background_reresolves_after_set() {
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp creation");
    let color = Signal::new(app.runtime(), Color::RED);
    let mut view = BgView {
        color: color.clone(),
    };

    let _ = app.render_view(&mut view).expect("first render");
    assert_eq!(app.scene().rects[0].color, [1.0, 0.0, 0.0, 1.0]);

    color.set(Color::BLUE);
    let _ = app.render_view(&mut view).expect("second render");
    assert_eq!(app.scene().rects[0].color, [0.0, 0.0, 1.0, 1.0]);
}

struct RadiusView {
    radius: Signal<f32>,
}

impl View for RadiusView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let r = self.radius.clone();
        Div::new()
            .background(Color::RED)
            .corner_radius(slate_framework::Reactive::dynamic(move || r.get()))
            .style(|s| s.width(40.0).height(40.0))
            .into_any()
    }
}

#[test]
fn signal_corner_radius_reresolves_after_set() {
    use slate_renderer::Lpx;
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp creation");
    let radius = Signal::new(app.runtime(), 4.0_f32);
    let mut view = RadiusView {
        radius: radius.clone(),
    };

    let _ = app.render_view(&mut view).expect("first render");
    assert_eq!(app.scene().rects[0].corner_radius, Lpx(4.0));

    radius.set(12.0);
    let _ = app.render_view(&mut view).expect("second render");
    assert_eq!(app.scene().rects[0].corner_radius, Lpx(12.0));
}
