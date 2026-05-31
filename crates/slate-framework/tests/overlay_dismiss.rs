//! Overlay dismiss integration tests (P3 Step 5).
//!
//! Proves the dismiss paths end-to-end through the headless render pipeline:
//! - **Esc** dismisses the top-most open overlay (popover/dropdown/dialog).
//! - **Outside-click** on a non-modal overlay dismisses without blocking, while
//!   clicks inside the content — or on the anchor (so the trigger can toggle) —
//!   pass through.
//! - A **modal scrim click** dismisses *and* blocks the click from the base tree
//!   (input-blocking); clicks inside the modal content pass through.
//!
//! Dismissal is signal-driven: the overlay wires `on_dismiss(|| open.set(false))`,
//! so firing it flips the caller's `open` signal and the overlay drops out of the
//! next rebuild. The headless `overlay_escape` / `overlay_click` hooks run the
//! same registry decision logic the windowed key/mouse dispatch calls; the pure
//! `click_outcome` cases are unit-tested in `elements/overlay/registry.rs`.

use slate_framework::reactive::Signal;
use slate_framework::{
    AnyElement, Bounds, Color, Div, HeadlessApp, IntoAny, Overlay, Placement, Point, View,
};

/// Width tag of the overlay's content focusable (locatable via `focusables()`
/// without predicting hashed ids; distinct from the outside button's width 80).
const CONTENT_W: f32 = 60.0;

/// A view with an always-present outside focusable (width 80) and an optional
/// overlay anchored to a fixed rect. The overlay's content is a focusable of
/// width [`CONTENT_W`] so the test can read its painted rect and click inside it.
struct DismissDemo {
    open: Signal<bool>,
    modal: bool,
    anchor: Bounds,
}

impl View for DismissDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let mut col = Div::new().style(|s| s.width(200.0).height(200.0).column()).child(
            Div::new()
                .focusable(true)
                .background(Color::RED)
                .style(|s| s.width(80.0).height(20.0)),
        );
        if self.open.get() {
            let open = self.open.clone();
            col = col.child(
                Overlay::new()
                    .modal(self.modal)
                    .depth(1000)
                    .anchor(self.anchor)
                    .placement(Placement::bottom())
                    .on_dismiss(move || open.set(false))
                    .child(
                        Div::new()
                            .focusable(true)
                            .background(Color::BLUE)
                            .style(|s| s.width(CONTENT_W).height(30.0)),
                    ),
            );
        }
        col.into_any()
    }
}

fn render(app: &mut HeadlessApp, open: &Signal<bool>, modal: bool, anchor: Bounds) {
    let mut view = DismissDemo {
        open: open.clone(),
        modal,
        anchor,
    };
    app.render_view(&mut view).expect("render");
}

/// The overlay content's painted bounds, or `None` if the overlay is closed.
fn content_bounds(app: &HeadlessApp) -> Option<Bounds> {
    app.focusables()
        .into_iter()
        .find(|(_, b)| (b.size.width - CONTENT_W).abs() < 0.5)
        .map(|(_, b)| b)
}

fn content_present(app: &HeadlessApp) -> bool {
    content_bounds(app).is_some()
}

fn center(b: Bounds) -> (f32, f32) {
    (b.origin.x + b.size.width / 2.0, b.origin.y + b.size.height / 2.0)
}

fn anchor_rect() -> Bounds {
    Bounds::from_origin_size(0.0, 0.0, 40.0, 12.0)
}

// ---------------------------------------------------------------------------
// Esc dismiss
// ---------------------------------------------------------------------------

#[test]
fn escape_dismisses_open_overlay() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let open = Signal::new(app.runtime(), true);
    render(&mut app, &open, false, anchor_rect());
    assert!(content_present(&app), "overlay open before Esc");

    assert!(app.overlay_escape(), "an overlay was open to dismiss");
    render(&mut app, &open, false, anchor_rect());
    assert!(!content_present(&app), "Esc dismissed the overlay");
}

#[test]
fn escape_is_noop_with_no_open_overlay() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let open = Signal::new(app.runtime(), false);
    render(&mut app, &open, false, anchor_rect());
    assert!(!app.overlay_escape(), "no overlay open → nothing to dismiss");
}

#[test]
fn escape_dismisses_modal_too() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let open = Signal::new(app.runtime(), true);
    render(&mut app, &open, true, anchor_rect());
    assert!(content_present(&app));

    assert!(app.overlay_escape());
    render(&mut app, &open, true, anchor_rect());
    assert!(!content_present(&app), "Esc dismissed the modal");
}

// ---------------------------------------------------------------------------
// Outside-click dismiss (non-modal)
// ---------------------------------------------------------------------------

#[test]
fn nonmodal_outside_click_dismisses_without_blocking() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let open = Signal::new(app.runtime(), true);
    render(&mut app, &open, false, anchor_rect());
    let c = content_bounds(&app).expect("overlay open");
    // Bottom-right corner — outside both the content rect and the anchor rect.
    assert!(!c.contains(Point::new(195.0, 195.0)));

    let (dismissed, blocked) = app.overlay_click(195.0, 195.0);
    assert!(dismissed, "outside click dismisses the popover");
    assert!(!blocked, "a non-modal popover must not block the click");

    render(&mut app, &open, false, anchor_rect());
    assert!(!content_present(&app), "popover gone after outside click");
}

#[test]
fn nonmodal_click_inside_content_passes() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let open = Signal::new(app.runtime(), true);
    render(&mut app, &open, false, anchor_rect());
    let (cx, cy) = center(content_bounds(&app).expect("overlay open"));

    let (dismissed, blocked) = app.overlay_click(cx, cy);
    assert!(!dismissed && !blocked, "click inside content passes through");

    render(&mut app, &open, false, anchor_rect());
    assert!(content_present(&app), "popover stays open on inside click");
}

#[test]
fn nonmodal_click_on_anchor_passes() {
    // Clicking the trigger must not also fire the outside-click dismiss, else the
    // trigger's own toggle and the dismiss would fight.
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let open = Signal::new(app.runtime(), true);
    let anchor = anchor_rect();
    render(&mut app, &open, false, anchor);

    let inside_anchor = (anchor.origin.x + 5.0, anchor.origin.y + 5.0);
    let (dismissed, blocked) = app.overlay_click(inside_anchor.0, inside_anchor.1);
    assert!(!dismissed && !blocked, "click on the anchor passes through");

    render(&mut app, &open, false, anchor);
    assert!(content_present(&app), "popover stays open on anchor click");
}

// ---------------------------------------------------------------------------
// Modal scrim click — dismiss + input-blocking
// ---------------------------------------------------------------------------

#[test]
fn modal_scrim_click_dismisses_and_blocks() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let open = Signal::new(app.runtime(), true);
    render(&mut app, &open, true, anchor_rect());
    let c = content_bounds(&app).expect("overlay open");
    assert!(!c.contains(Point::new(195.0, 195.0)));

    let (dismissed, blocked) = app.overlay_click(195.0, 195.0);
    assert!(dismissed, "scrim click dismisses the modal");
    assert!(blocked, "scrim click is swallowed (base tree gets nothing)");

    render(&mut app, &open, true, anchor_rect());
    assert!(!content_present(&app), "modal gone after scrim click");
}

#[test]
fn modal_click_inside_content_passes() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let open = Signal::new(app.runtime(), true);
    render(&mut app, &open, true, anchor_rect());
    let (cx, cy) = center(content_bounds(&app).expect("overlay open"));

    let (dismissed, blocked) = app.overlay_click(cx, cy);
    assert!(!dismissed && !blocked, "click inside the dialog passes through");

    render(&mut app, &open, true, anchor_rect());
    assert!(content_present(&app), "modal stays open on inside click");
}

// ---------------------------------------------------------------------------
// Overlay without on_dismiss
// ---------------------------------------------------------------------------

#[test]
fn modal_without_on_dismiss_still_blocks_scrim_clicks() {
    // A modal that opts out of dismissal must still block base-tree input.
    struct NoDismiss {
        anchor: Bounds,
    }
    impl View for NoDismiss {
        fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
            Div::new()
                .style(|s| s.width(200.0).height(200.0).column())
                .child(
                    Overlay::new()
                        .modal(true)
                        .depth(1000)
                        .anchor(self.anchor)
                        .placement(Placement::bottom())
                        .child(
                            Div::new()
                                .focusable(true)
                                .background(Color::BLUE)
                                .style(|s| s.width(CONTENT_W).height(30.0)),
                        ),
                )
                .into_any()
        }
    }
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let mut view = NoDismiss {
        anchor: anchor_rect(),
    };
    app.render_view(&mut view).expect("render");

    let (dismissed, blocked) = app.overlay_click(195.0, 195.0);
    assert!(!dismissed, "no callback → nothing dismissed");
    assert!(blocked, "modal still blocks the click without an on_dismiss");
}
