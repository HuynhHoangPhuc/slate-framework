//! Overlay renderer-bridge regression tests.
//!
//! Proves the two guarantees of the overlay paint mechanism:
//! 1. **Clip escape + on-top** — an Overlay declared *inside* a clipped
//!    ScrollView paints its content into a high-`depth`, **unclipped** scene
//!    layer, so it escapes the ancestor scroll-clip and the production
//!    depth-sort (`depth_sorted_layers`) draws it last (on top).
//! 2. **No flow disturbance** — because the overlay node is laid out absolutely,
//!    declaring it between two siblings does not shift them.

use slate_framework::{
    AnyElement, Bounds, Color, Div, HeadlessApp, IntoAny, Overlay, Placement, ScrollView, View,
};
use slate_renderer::Lpx;
use slate_renderer::depth_sorted_layers;

const OVERLAY_DEPTH: i32 = 500;

/// A 100×100 clipped ScrollView holding a tall row plus an overlay anchored to a
/// rect inside it. Overlay content is a 50×40 blue box.
struct ClippedOverlayDemo;

impl View for ClippedOverlayDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        ScrollView::new()
            .style(|s| s.width(100.0).height(100.0).column())
            .child(
                Div::new()
                    .background(Color::RED)
                    .style(|s| s.width(100.0).height(200.0)),
            )
            .child(
                Overlay::new()
                    .anchor(Bounds::from_origin_size(20.0, 40.0, 30.0, 20.0))
                    .placement(Placement::bottom())
                    .depth(OVERLAY_DEPTH)
                    .child(
                        Div::new()
                            .background(Color::BLUE)
                            .style(|s| s.width(50.0).height(40.0)),
                    ),
            )
            .into_any()
    }
}

#[test]
fn overlay_paints_in_unclipped_high_depth_layer() {
    // Root sized 200×200 so the anchor solver has room and never clamps.
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let mut view = ClippedOverlayDemo;
    let _ = app.render_view(&mut view).expect("render");

    let scene = app.scene();

    // The ScrollView still emits a clipped layer (its own content).
    assert!(
        scene.layers.iter().any(|l| l.clip.is_some()),
        "ScrollView must still clip its content"
    );

    // Exactly one overlay layer at OVERLAY_DEPTH, and it is unclipped — the
    // clip-escape guarantee: it ignores the enclosing ScrollView scissor.
    let overlay_layers: Vec<&_> = scene
        .layers
        .iter()
        .filter(|l| l.depth == OVERLAY_DEPTH)
        .collect();
    assert_eq!(overlay_layers.len(), 1, "one overlay layer expected");
    let overlay_layer = overlay_layers[0];
    assert!(
        overlay_layer.clip.is_none(),
        "overlay layer must be unclipped (escapes ancestor clip)"
    );

    // The blue content rect lives in that layer, at the anchor-solved position:
    // anchor bottom (y 40+20) + gap 4 = 64; centered x = 20 + 30/2 - 50/2 = 10.
    let start = overlay_layer.rects.start as usize;
    assert!(start < scene.rects.len(), "overlay layer has a rect");
    let r = scene.rects[start].rect;
    assert_eq!(r[0], Lpx(10.0), "solved x (centered on anchor)");
    assert_eq!(r[1], Lpx(64.0), "solved y (below anchor + gap)");
    assert_eq!(r[2], Lpx(50.0));
    assert_eq!(r[3], Lpx(40.0));

    // Production depth-sort draws the overlay layer last (on top of everything).
    let depths: Vec<i32> = scene.layers.iter().map(|l| l.depth).collect();
    let order = depth_sorted_layers(&depths);
    let last = *order.last().expect("non-empty layer order");
    assert_eq!(
        scene.layers[last].depth,
        OVERLAY_DEPTH,
        "overlay layer must sort on top"
    );
}

/// Column of [row A, (optional) Overlay, row B (green)]. With `with_overlay`
/// false this is the flow baseline; true inserts an absolute overlay between the
/// rows. The green row's position must be identical either way — the absolute
/// overlay occupies no flow space.
struct FlowDemo {
    with_overlay: bool,
}

impl View for FlowDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let mut col = Div::new()
            .style(|s| s.width(100.0).height(200.0).column())
            .child(
                Div::new()
                    .background(Color::RED)
                    .style(|s| s.width(100.0).height(30.0)),
            );
        if self.with_overlay {
            col = col.child(
                Overlay::new()
                    .anchor(Bounds::from_origin_size(0.0, 0.0, 10.0, 10.0))
                    .child(
                        Div::new()
                            .background(Color::BLUE)
                            .style(|s| s.width(50.0).height(40.0)),
                    ),
            );
        }
        col.child(
            Div::new()
                .background(Color::GREEN)
                .style(|s| s.width(100.0).height(30.0)),
        )
        .into_any()
    }
}

/// The Y of the (unique) green row rect in the freshly-rendered scene.
fn green_row_y(app: &HeadlessApp) -> Lpx {
    app.scene()
        .rects
        .iter()
        .find(|r| r.color == [0.0, 1.0, 0.0, 1.0])
        .expect("green row present")
        .rect[1]
}

#[test]
fn absolute_overlay_does_not_disturb_sibling_flow() {
    let mut app = HeadlessApp::with_scale_factor(100, 200, 1.0).expect("app");

    let mut baseline = FlowDemo { with_overlay: false };
    let _ = app.render_view(&mut baseline).expect("baseline render");
    let baseline_y = green_row_y(&app);

    let mut with_overlay = FlowDemo { with_overlay: true };
    let _ = app.render_view(&mut with_overlay).expect("overlay render");
    let overlay_y = green_row_y(&app);

    assert_eq!(
        overlay_y, baseline_y,
        "green row must not move when an absolute overlay is inserted before it"
    );
}

/// Depth of the layer that owns scene rect `idx`.
fn rect_layer_depth(app: &HeadlessApp, idx: usize) -> i32 {
    app.scene()
        .layers
        .iter()
        .find(|l| (l.rects.start as usize..l.rects.end as usize).contains(&idx))
        .expect("rect belongs to a layer")
        .depth
}

/// An overlay that paints nothing (no content) followed by a normal sibling.
/// The sibling must stay at base depth 0 — the empty overlay layer must not
/// leak its high depth onto it.
struct EmptyOverlayThenSibling;

impl View for EmptyOverlayThenSibling {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new()
            .style(|s| s.column())
            .child(
                Overlay::new()
                    .depth(700)
                    .anchor(Bounds::from_origin_size(0.0, 0.0, 10.0, 10.0)),
            )
            .child(
                Div::new()
                    .background(Color::GREEN)
                    .style(|s| s.width(40.0).height(20.0)),
            )
            .into_any()
    }
}

#[test]
fn empty_overlay_does_not_leak_depth_to_following_sibling() {
    let mut app = HeadlessApp::with_scale_factor(100, 100, 1.0).expect("app");
    let mut view = EmptyOverlayThenSibling;
    let _ = app.render_view(&mut view).expect("render");

    // The green sibling is the only rect; it must sit at depth 0, not 700.
    let green = app
        .scene()
        .rects
        .iter()
        .position(|r| r.color == [0.0, 1.0, 0.0, 1.0])
        .expect("green sibling present");
    assert_eq!(
        rect_layer_depth(&app, green),
        0,
        "sibling after an empty overlay must stay at base depth 0"
    );
}

/// KNOWN LIMITATION (pinning test). An overlay whose content is itself clipped
/// (here a ScrollView) does not yet lift the clipped portion to the overlay
/// depth: the renderer clip path opens its layer at depth 0. This pins the
/// current behavior so it is visible when overlay-with-scroll support lands and
/// this assertion is intentionally updated. See `elements/overlay` docs.
struct OverlayWithClippedBody;

impl View for OverlayWithClippedBody {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new()
            .style(|s| s.column())
            .child(
                Overlay::new()
                    .depth(600)
                    .anchor(Bounds::from_origin_size(0.0, 0.0, 10.0, 10.0))
                    .child(
                        ScrollView::new()
                            .style(|s| s.width(50.0).height(50.0).column())
                            .child(
                                Div::new()
                                    .background(Color::BLUE)
                                    .style(|s| s.width(50.0).height(20.0)),
                            ),
                    ),
            )
            .into_any()
    }
}

#[test]
fn clipped_overlay_body_stays_at_depth_zero_known_limitation() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let mut view = OverlayWithClippedBody;
    let _ = app.render_view(&mut view).expect("render");

    let blue = app
        .scene()
        .rects
        .iter()
        .position(|r| r.color == [0.0, 0.0, 1.0, 1.0])
        .expect("blue body present");
    // Limitation: the clipped body lands at depth 0, NOT the overlay's 600.
    assert_eq!(
        rect_layer_depth(&app, blue),
        0,
        "clipped overlay body currently does not inherit overlay depth"
    );
}
