//! Anchor-to-element coverage for the Overlay element.
//!
//! Asserts that `Div::track_bounds` captures an element's painted bounds into a
//! caller-owned `Signal<Bounds>`, that the capture is stable across identical
//! re-renders (the value-level guarantee behind the guarded write that keeps the
//! Strategy-A rebuild loop from spinning), and that `Overlay::anchor` fed by that
//! same signal renders on top — i.e. an overlay follows a live element rect with
//! no hardcoded coordinates.
//!
//! Drives the real headless render path (`HeadlessApp::render_view`), mirroring
//! the `reactive_div_modifiers` integration test.

use slate_framework::reactive::Signal;
use slate_framework::{AnyElement, Bounds, Color, Div, HeadlessApp, IntoAny, Overlay, Placement, View};

/// A view with a fixed-size "button" that reports its painted rect into
/// `anchor`, optionally followed by an Overlay anchored to that same signal.
struct AnchorView {
    anchor: Signal<Bounds>,
    with_overlay: bool,
}

impl View for AnchorView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let mut root = Div::new().style(|s| s.column().padding_all(20.0)).child(
            Div::new()
                .background(Color::from_hex("#89b4fa").unwrap_or(Color::BLUE))
                .style(|s| s.width(100.0).height(40.0))
                .track_bounds(self.anchor.clone()),
        );

        if self.with_overlay {
            root = root.child(
                Overlay::new()
                    .anchor(self.anchor.clone())
                    .placement(Placement::bottom())
                    .depth(1000)
                    .child(
                        Div::new()
                            .background(Color::from_hex("#cdd6f4").unwrap_or(Color::WHITE))
                            .style(|s| s.width(80.0).height(20.0)),
                    ),
            );
        }

        root.into_any()
    }
}

/// Render an `AnchorView` once through the headless pipeline.
fn render(app: &mut HeadlessApp, anchor: &Signal<Bounds>, with_overlay: bool) {
    let mut view = AnchorView {
        anchor: anchor.clone(),
        with_overlay,
    };
    let _ = app.render_view(&mut view).expect("render");
}

#[test]
fn track_bounds_captures_painted_bounds() {
    let mut app = HeadlessApp::with_scale_factor(400, 300, 1.0).expect("app");
    let anchor = Signal::new(app.runtime(), Bounds::ZERO);
    render(&mut app, &anchor, false);

    // Fixed-size child inside 20px padding → origin (20,20), size (100,40).
    let b = anchor.get_untracked();
    assert_eq!(b.origin.x, 20.0);
    assert_eq!(b.origin.y, 20.0);
    assert_eq!(b.size.width, 100.0);
    assert_eq!(b.size.height, 40.0);
}

#[test]
fn track_bounds_stable_across_rerenders() {
    let mut app = HeadlessApp::with_scale_factor(400, 300, 1.0).expect("app");
    let anchor = Signal::new(app.runtime(), Bounds::ZERO);

    render(&mut app, &anchor, false);
    let first = anchor.get_untracked();
    assert_ne!(first, Bounds::ZERO, "bounds captured after first render");

    // Identical layout re-rendered: the guarded write keeps the value stable
    // (it fires only on change), so the anchor never drifts frame-to-frame.
    render(&mut app, &anchor, false);
    assert_eq!(
        anchor.get_untracked(),
        first,
        "bounds stable across identical re-renders"
    );
}

#[test]
fn overlay_anchors_to_tracked_element() {
    let mut app = HeadlessApp::with_scale_factor(400, 300, 1.0).expect("app");
    let anchor = Signal::new(app.runtime(), Bounds::ZERO);

    // Pass 1 populates `anchor` from the tracked button's painted rect.
    render(&mut app, &anchor, true);
    let captured = anchor.get_untracked();
    assert_ne!(captured, Bounds::ZERO, "anchor populated from tracked element");

    // Pass 2: the overlay resolves its anchor from the now-populated signal and
    // paints in a higher depth bucket than the base tree.
    render(&mut app, &anchor, true);
    let max_depth = app.scene().layers.iter().map(|l| l.depth).max().unwrap_or(0);
    assert!(
        max_depth > 0,
        "overlay anchored to a tracked element should paint on top"
    );
    assert_eq!(
        anchor.get_untracked(),
        captured,
        "anchor stays locked to the tracked element rect"
    );
}
