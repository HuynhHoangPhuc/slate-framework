//! Modal Overlay integration tests (P3 Step 4).
//!
//! Proves the three modal guarantees end-to-end through the headless render +
//! focus pipeline:
//! 1. **Scrim** — a modal paints a full-viewport dim into its high-depth overlay
//!    layer, behind the content but above the base tree.
//! 2. **Focus trap** — while a modal is open, Tab cycling is confined to the
//!    overlay subtree and never escapes to outside focusables.
//! 3. **Focus restore** — opening a modal auto-focuses its first focusable and
//!    saves the prior focus; closing it restores that prior focus.
//!
//! The pure trap/reconcile logic is unit-tested in `src/focus.rs`; these tests
//! verify the `Overlay::modal` wiring (scrim paint + trap-scope registration)
//! and the per-frame reconcile via the headless harness.

use slate_framework::{
    AnyElement, Bounds, Color, Div, HeadlessApp, IntoAny, Overlay, Placement, View,
};
use slate_renderer::Lpx;

/// Scrim color emitted by a modal overlay (linear premultiplied black @ 0.4).
const SCRIM_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.4];

/// A column with an always-present outside focusable (width 80) and an optional
/// overlay holding two inside focusables (widths 50, 51). `modal` toggles the
/// overlay's modality. Widths make each focusable findable without predicting
/// hashed ElementIds.
struct ModalDemo {
    open: bool,
    modal: bool,
}

impl View for ModalDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let mut col = Div::new().style(|s| s.width(200.0).height(200.0).column()).child(
            Div::new()
                .focusable(true)
                .background(Color::RED)
                .style(|s| s.width(80.0).height(20.0)),
        );
        if self.open {
            col = col.child(
                Overlay::new()
                    .modal(self.modal)
                    .depth(1000)
                    .anchor(Bounds::from_origin_size(0.0, 0.0, 10.0, 10.0))
                    .placement(Placement::bottom())
                    .child(
                        Div::new()
                            .style(|s| s.column())
                            .child(
                                Div::new()
                                    .focusable(true)
                                    .background(Color::BLUE)
                                    .style(|s| s.width(50.0).height(20.0)),
                            )
                            .child(
                                Div::new()
                                    .focusable(true)
                                    .background(Color::GREEN)
                                    .style(|s| s.width(51.0).height(20.0)),
                            ),
                    ),
            );
        }
        col.into_any()
    }
}

/// Find the focusable whose painted width matches `w` (the demo's id tag).
fn focusable_by_width(app: &HeadlessApp, w: f32) -> slate_framework::ElementId {
    app.focusables()
        .into_iter()
        .find(|(_, b)| (b.size.width - w).abs() < 0.5)
        .unwrap_or_else(|| panic!("focusable of width {w} not found"))
        .0
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

#[test]
fn modal_paints_fullviewport_scrim_in_overlay_layer() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let mut view = ModalDemo {
        open: true,
        modal: true,
    };
    app.render_view(&mut view).expect("render");

    let scene = app.scene();
    let scrim_idx = scene
        .rects
        .iter()
        .position(|r| r.color == SCRIM_COLOR)
        .expect("modal scrim rect present");

    // Scrim covers the whole viewport.
    assert_eq!(
        scene.rects[scrim_idx].rect,
        [Lpx(0.0), Lpx(0.0), Lpx(200.0), Lpx(200.0)],
        "scrim must cover the full viewport"
    );
    // Scrim lives in the overlay's high-depth layer (above the base tree).
    assert_eq!(
        rect_layer_depth(&app, scrim_idx),
        1000,
        "scrim must sit in the overlay depth layer"
    );
    // Scrim is drawn before the blue/green content rects (behind them).
    let blue_idx = scene
        .rects
        .iter()
        .position(|r| r.color == [0.0, 0.0, 1.0, 1.0])
        .expect("inside content present");
    assert!(scrim_idx < blue_idx, "scrim must paint behind the content");
}

#[test]
fn non_modal_overlay_paints_no_scrim() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let mut view = ModalDemo {
        open: true,
        modal: false,
    };
    app.render_view(&mut view).expect("render");

    assert!(
        !app.scene().rects.iter().any(|r| r.color == SCRIM_COLOR),
        "a non-modal overlay must not paint a scrim"
    );
}

#[test]
fn modal_traps_tab_focus_inside_overlay() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let mut view = ModalDemo {
        open: true,
        modal: true,
    };
    app.render_view(&mut view).expect("render");

    let outside = focusable_by_width(&app, 80.0);
    let inside_a = focusable_by_width(&app, 50.0);
    let inside_b = focusable_by_width(&app, 51.0);

    // Reconcile = the per-frame lifecycle: auto-focus the modal's first element.
    app.reconcile_modal_focus();
    assert_eq!(app.focused(), Some(inside_a), "modal auto-focuses first");

    // Tab cycles only within the modal, wrapping — never escaping to `outside`.
    assert_eq!(app.tab(false), Some(inside_b));
    assert_eq!(app.tab(false), Some(inside_a), "wraps inside the modal");
    assert_eq!(app.tab(true), Some(inside_b), "Shift+Tab also trapped");
    assert_ne!(app.focused(), Some(outside), "focus never reaches outside");
}

#[test]
fn non_modal_overlay_does_not_trap_tab() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let mut view = ModalDemo {
        open: true,
        modal: false,
    };
    app.render_view(&mut view).expect("render");
    app.reconcile_modal_focus(); // no modal → no auto-focus, empty stack

    let outside = focusable_by_width(&app, 80.0);
    // With no trap, Tab from no-focus lands on the first focusable in the whole
    // tree (the outside button registers first).
    assert_eq!(
        app.tab(false),
        Some(outside),
        "non-modal overlay leaves Tab walking the whole tree"
    );
}

#[test]
fn modal_close_restores_prior_focus() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");

    // Frame 1: no modal; focus the outside button.
    let mut closed = ModalDemo {
        open: false,
        modal: true,
    };
    app.render_view(&mut closed).expect("render closed");
    let outside = focusable_by_width(&app, 80.0);
    assert!(app.set_focus(outside), "outside focusable");
    app.reconcile_modal_focus();

    // Frame 2: modal opens — prior focus saved, focus moved inside.
    let mut open = ModalDemo {
        open: true,
        modal: true,
    };
    app.render_view(&mut open).expect("render open");
    let inside_a = focusable_by_width(&app, 50.0);
    app.reconcile_modal_focus();
    assert_eq!(app.focused(), Some(inside_a), "focus moved into modal");

    // Frame 3: modal closes — prior (outside) focus restored.
    app.render_view(&mut closed).expect("render closed again");
    app.reconcile_modal_focus();
    assert_eq!(
        app.focused(),
        Some(outside),
        "closing the modal restores the prior focus"
    );
}
