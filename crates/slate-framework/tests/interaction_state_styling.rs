//! Interaction-state styling regression tests.
//!
//! Verifies the P1 foundation: per-element hover / active(pressed) / disabled
//! style overrides resolve correctly at paint time, the no-override fast path
//! stays byte-identical, the resolution order is base → hover → active →
//! disabled, signal-backed state overrides re-resolve under the whole-view
//! rebuild model, and a disabled element reports `is_disabled` to AccessKit.

use slate_framework::reactive::Signal;
use slate_framework::{AnyElement, Color, Div, HeadlessApp, IntoAny, View};

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const GRAY: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// A 40×40 Div filling the viewport, with the given builder applied.
fn full_div(f: impl FnOnce(Div) -> Div) -> AnyElement {
    f(Div::new().style(|s| s.width(40.0).height(40.0))).into_any()
}

#[test]
fn fast_path_ignores_cursor_when_no_overrides() {
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp");
    // Cursor parked over the element, but no hover override declared.
    app.set_cursor(Some((20.0, 20.0)));
    let _ = app
        .render(full_div(|d| d.background(Color(RED))))
        .expect("render");
    assert_eq!(app.scene().rects[0].color, RED, "base color unchanged");
}

#[test]
fn hover_override_applies_under_cursor() {
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp");
    let ui = || full_div(|d| d.background(Color(RED)).hover(|s| s.background(Color(BLUE))));

    // No cursor → base color.
    let _ = app.render(ui()).expect("render");
    assert_eq!(app.scene().rects[0].color, RED);

    // Cursor over element → hover color.
    app.set_cursor(Some((20.0, 20.0)));
    let _ = app.render(ui()).expect("render");
    assert_eq!(app.scene().rects[0].color, BLUE);

    // Cursor off element (outside 40×40) → back to base.
    app.set_cursor(Some((100.0, 100.0)));
    let _ = app.render(ui()).expect("render");
    assert_eq!(app.scene().rects[0].color, RED);
}

#[test]
fn active_override_applies_while_pressing() {
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp");
    let ui = || {
        full_div(|d| {
            d.background(Color(RED))
                .hover(|s| s.background(Color(BLUE)))
                .active(|s| s.background(Color(GREEN)))
        })
    };

    app.set_cursor(Some((20.0, 20.0)));
    app.set_pressing(true);
    let _ = app.render(ui()).expect("render");
    assert_eq!(app.scene().rects[0].color, GREEN, "pressed wins over hover");

    // Release → hover only.
    app.set_pressing(false);
    let _ = app.render(ui()).expect("render");
    assert_eq!(app.scene().rects[0].color, BLUE);
}

#[test]
fn disabled_override_wins_over_hover() {
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp");
    app.set_cursor(Some((20.0, 20.0)));
    app.set_pressing(true);
    let _ = app
        .render(full_div(|d| {
            d.background(Color(RED))
                .hover(|s| s.background(Color(BLUE)))
                .active(|s| s.background(Color(GREEN)))
                .disabled(true)
                .disabled_style(|s| s.background(Color(GRAY)))
        }))
        .expect("render");
    assert_eq!(
        app.scene().rects[0].color,
        GRAY,
        "disabled overrides hover and active"
    );
}

#[test]
fn corner_radius_override_resolves() {
    use slate_renderer::Lpx;
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp");
    app.set_cursor(Some((20.0, 20.0)));
    let _ = app
        .render(full_div(|d| {
            d.background(Color(RED))
                .corner_radius(4.0)
                .hover(|s| s.corner_radius(12.0))
        }))
        .expect("render");
    assert_eq!(app.scene().rects[0].corner_radius, Lpx(12.0));
}

#[test]
fn disabled_reported_to_accesskit() {
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp");
    let _ = app
        .render(full_div(|d| d.background(Color(RED)).disabled(true)))
        .expect("render");
    let nodes = app.a11y_nodes();
    assert!(!nodes.is_empty(), "a11y tree built");
    assert!(nodes[0].info.is_disabled, "disabled reported to AccessKit");
}

#[test]
fn enabled_not_reported_disabled() {
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp");
    let _ = app
        .render(full_div(|d| d.background(Color(RED))))
        .expect("render");
    assert!(!app.a11y_nodes()[0].info.is_disabled);
}

/// View whose hover override is backed by a signal — exercises the reactive
/// state-prop path beyond `background` (Strategy-A whole-view rebuild).
struct ReactiveHoverView {
    hover_color: Signal<Color>,
}

impl View for ReactiveHoverView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let c = self.hover_color.clone();
        Div::new()
            .background(Color(RED))
            .hover(move |s| s.background(c.clone()))
            .style(|s| s.width(40.0).height(40.0))
            .into_any()
    }
}

#[test]
fn reactive_hover_override_reresolves_after_set() {
    let mut app = HeadlessApp::with_scale_factor(40, 40, 1.0).expect("HeadlessApp");
    app.set_cursor(Some((20.0, 20.0)));
    let hover_color = Signal::new(app.runtime(), Color(BLUE));
    let mut view = ReactiveHoverView {
        hover_color: hover_color.clone(),
    };

    let _ = app.render_view(&mut view).expect("first render");
    assert_eq!(app.scene().rects[0].color, BLUE);

    hover_color.set(Color(GREEN));
    let _ = app.render_view(&mut view).expect("second render");
    assert_eq!(app.scene().rects[0].color, GREEN, "signal-backed hover re-resolves");
}
