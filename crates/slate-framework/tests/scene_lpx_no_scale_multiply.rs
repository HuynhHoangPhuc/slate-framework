//! Element-level regression test for the lpx scene wire format.
//!
//! Bug 2 (top-left-quadrant draws on Retina) was caused by `div.rs` doing
//! `* scale` before pushing `RectInstance`. After the lpx migration, elements
//! push lpx values unchanged and the renderer's viewport uniform maps lpx→NDC.
//!
//! This test paints a `Div::new().background(...)` at scale=1.0 and scale=2.0
//! and asserts that the pushed `RectInstance.rect` carries identical lpx
//! coordinates at both scales — never the doubled physical-pixel values.

use slate_framework::{Div, HeadlessApp, IntoAny};
use slate_renderer::Lpx;

#[test]
fn div_background_rect_is_lpx_at_scale_1() {
    let mut app = HeadlessApp::with_scale_factor(100, 50, 1.0).expect("HeadlessApp creation");
    let _ = app
        .render(
            Div::new()
                .background([1.0, 0.0, 0.0, 1.0])
                .style(|s| s.width(100.0).height(50.0))
                .into_any(),
        )
        .expect("render");

    let scene = app.scene();
    assert!(!scene.rects.is_empty(), "expected at least one rect");
    let r = scene.rects[0].rect;
    assert_eq!(
        r,
        [Lpx(0.0), Lpx(0.0), Lpx(100.0), Lpx(50.0)],
        "rect must be in lpx (no scale multiply)"
    );
}

#[test]
fn div_background_rect_is_lpx_at_scale_2() {
    let mut app = HeadlessApp::with_scale_factor(100, 50, 2.0).expect("HeadlessApp creation");
    let _ = app
        .render(
            Div::new()
                .background([1.0, 0.0, 0.0, 1.0])
                .style(|s| s.width(100.0).height(50.0))
                .into_any(),
        )
        .expect("render");

    let scene = app.scene();
    assert!(!scene.rects.is_empty(), "expected at least one rect");
    let r = scene.rects[0].rect;
    assert_eq!(
        r,
        [Lpx(0.0), Lpx(0.0), Lpx(100.0), Lpx(50.0)],
        "rect must be lpx independent of scale_factor (not [0,0,200,100])"
    );
}

#[test]
fn div_background_rect_lpx_identical_across_scales() {
    let render_at = |scale: f64| -> [Lpx; 4] {
        let mut app = HeadlessApp::with_scale_factor(80, 40, scale).expect("HeadlessApp creation");
        let _ = app
            .render(
                Div::new()
                    .background([0.0, 1.0, 0.0, 1.0])
                    .style(|s| s.width(80.0).height(40.0))
                    .into_any(),
            )
            .expect("render");
        app.scene().rects[0].rect
    };
    assert_eq!(render_at(1.0), render_at(2.0));
    assert_eq!(render_at(1.0), render_at(1.5));
}
