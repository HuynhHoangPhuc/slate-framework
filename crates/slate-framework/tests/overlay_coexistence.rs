//! Overlay coexistence with scroll re-anchoring and IME composition (P3 Step 6).
//!
//! - **Scroll re-anchor**: an overlay anchored to a `Signal<Bounds>` follows the
//!   anchor when it moves. A scrolling container reports its trigger's new rect
//!   through `Div::track_bounds`, which writes that signal — so scrolling the
//!   anchor recomputes the overlay's solved position on the next frame. This
//!   drives the signal directly (the unit of recompute) rather than a full
//!   ScrollView, which is covered by the P2 scroll suite.
//! - **IME coexistence**: opening and dismissing an overlay while an IME-capable
//!   element is in the tree (with a live composition state) must not panic and
//!   must leave the IME element's state intact — overlay open/close is a plain
//!   signal write + rebuild, never a re-entrant registry mutation (ADR-001).
//!
//! Windowed key/mouse dispatch borrow-safety (clone the dismiss `Rc`, drop the
//! borrow, then invoke) is structural and exercised by the Win11 dispatch path;
//! these macOS-runnable tests lock the render-pipeline coexistence facts.

use slate_framework::ime::{ImeState, Preedit};
use slate_framework::reactive::Signal;
use slate_framework::{
    AnyElement, Bounds, Color, Div, HeadlessApp, IntoAny, Overlay, Placement, View,
};

const CONTENT_W: f32 = 60.0;

// ---------------------------------------------------------------------------
// Scroll re-anchor
// ---------------------------------------------------------------------------

/// An overlay anchored to a caller-owned `Signal<Bounds>` (what `track_bounds`
/// writes for a tracked trigger). Moving the signal models the trigger scrolling.
struct ReanchorView {
    anchor: Signal<Bounds>,
}

impl View for ReanchorView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new()
            .style(|s| s.width(200.0).height(400.0).column())
            .child(
                Overlay::new()
                    .anchor(self.anchor.clone())
                    .placement(Placement::bottom())
                    .depth(1000)
                    .gap(4.0)
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

fn content_bounds(app: &HeadlessApp) -> Bounds {
    app.focusables()
        .into_iter()
        .find(|(_, b)| (b.size.width - CONTENT_W).abs() < 0.5)
        .map(|(_, b)| b)
        .expect("overlay content focusable present")
}

#[test]
fn overlay_reanchors_when_anchor_scrolls() {
    let mut app = HeadlessApp::with_scale_factor(200, 400, 1.0).expect("app");
    let anchor = Signal::new(app.runtime(), Bounds::from_origin_size(0.0, 50.0, 40.0, 12.0));
    let mut view = ReanchorView {
        anchor: anchor.clone(),
    };

    app.render_view(&mut view).expect("render 1");
    let before = content_bounds(&app);
    // Placement::bottom() + gap 4 → content sits just below the anchor's bottom.
    assert!(
        (before.origin.y - (50.0 + 12.0 + 4.0)).abs() < 0.5,
        "content anchored below the trigger (got y={})",
        before.origin.y
    );

    // Trigger scrolls up by 30px (as a ScrollView would report via track_bounds).
    anchor.set(Bounds::from_origin_size(0.0, 20.0, 40.0, 12.0));
    app.render_view(&mut view).expect("render 2");
    let after = content_bounds(&app);

    assert!(
        (after.origin.y - (before.origin.y - 30.0)).abs() < 0.5,
        "overlay re-anchored with the scrolled trigger: before y={}, after y={}",
        before.origin.y,
        after.origin.y
    );
}

// ---------------------------------------------------------------------------
// IME coexistence
// ---------------------------------------------------------------------------

/// A view with an always-present IME-capable field and an optional overlay, so a
/// composition can be live while the overlay opens/closes.
struct ImeCoexistView {
    open: Signal<bool>,
}

impl View for ImeCoexistView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let mut col = Div::new().style(|s| s.width(200.0).height(200.0).column()).child(
            // IME-capable field (re-registers an ImeState entry each prepaint).
            Div::new()
                .focusable(true)
                .ime_capable(true)
                .background(Color::RED)
                .style(|s| s.width(120.0).height(24.0)),
        );
        if self.open.get() {
            let open = self.open.clone();
            col = col.child(
                Overlay::new()
                    .depth(1000)
                    .anchor(Bounds::from_origin_size(0.0, 0.0, 40.0, 12.0))
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

fn ime_field_id(app: &HeadlessApp) -> slate_framework::ElementId {
    app.focusables()
        .into_iter()
        .find(|(_, b)| (b.size.width - 120.0).abs() < 0.5)
        .map(|(id, _)| id)
        .expect("ime field focusable present")
}

fn overlay_present(app: &HeadlessApp) -> bool {
    app.focusables()
        .into_iter()
        .any(|(_, b)| (b.size.width - CONTENT_W).abs() < 0.5)
}

#[test]
fn dismissing_overlay_during_composition_does_not_panic() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let open = Signal::new(app.runtime(), true);
    let mut view = ImeCoexistView { open: open.clone() };

    // Frame 1: overlay open + an active composition on the IME field.
    app.render_view(&mut view).expect("render open");
    assert!(overlay_present(&app), "overlay open before dismiss");
    let field = ime_field_id(&app);
    app.set_ime_state(
        field,
        ImeState {
            preedit: Some(Preedit {
                text: "に".to_string(),
                cursor_byte_offset: 3,
                ..Preedit::default()
            }),
            ..ImeState::default()
        },
    );

    // Dismiss the overlay mid-composition — a deferred signal write, no panic.
    assert!(app.overlay_escape(), "overlay dismissed");

    // Frame 2: overlay gone, IME field still present and re-registers cleanly.
    app.render_view(&mut view).expect("render after dismiss");
    assert!(!overlay_present(&app), "overlay closed after dismiss");
    let field2 = ime_field_id(&app);
    assert_eq!(field, field2, "IME field stable across overlay teardown");
}

#[test]
fn opening_overlay_over_ime_field_does_not_panic() {
    let mut app = HeadlessApp::with_scale_factor(200, 200, 1.0).expect("app");
    let open = Signal::new(app.runtime(), false);
    let mut view = ImeCoexistView { open: open.clone() };

    // Frame 1: no overlay; compose on the field.
    app.render_view(&mut view).expect("render closed");
    let field = ime_field_id(&app);
    app.set_ime_state(
        field,
        ImeState {
            preedit: Some(Preedit {
                text: "ほ".to_string(),
                cursor_byte_offset: 3,
                ..Preedit::default()
            }),
            ..ImeState::default()
        },
    );

    // Frame 2: open the overlay over the composing field — no panic, both present.
    open.set(true);
    app.render_view(&mut view).expect("render with overlay");
    assert!(overlay_present(&app), "overlay opened over the IME field");
    assert_eq!(field, ime_field_id(&app), "IME field still present");
}
