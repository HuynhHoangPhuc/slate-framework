//! ScrollView focus-driven scroll-into-view tests (step 5).
//!
//! Proves the `AccessibilityAction::ScrollIntoView` semantics for the focus
//! path: when keyboard focus lands on a focusable descendant outside the
//! viewport, the ScrollView sets its caller-owned offset signal so the element
//! is revealed on the next frame. Also proves the reveal fires only on a focus
//! *change* (it does not fight a manual scroll once it has reacted).

use slate_framework::reactive::Signal;
use slate_framework::types::ElementId;
use slate_framework::{AnyElement, Color, Div, HeadlessApp, IntoAny, ScrollView, View};

const VIEWPORT_H: f32 = 100.0;
const VIEWPORT_W: f32 = 120.0;
const ROW_H: f32 = 60.0;
const N_ROWS: usize = 4; // content 240px in a 100px viewport → max scroll 140.

/// Four stacked focusable rows inside a 100px viewport.
fn scroll_view(offset: Signal<f32>) -> AnyElement {
    let mut sv = ScrollView::new()
        .offset(offset)
        .style(|s| s.width(VIEWPORT_W).height(VIEWPORT_H).column());
    for i in 0..N_ROWS {
        let color = if i % 2 == 0 { Color::RED } else { Color::BLUE };
        sv = sv.child(
            Div::new()
                .background(color)
                .focusable(true)
                .style(|s| s.width(VIEWPORT_W).height(ROW_H)),
        );
    }
    sv.into_any()
}

struct ScrollDemo {
    offset: Signal<f32>,
}

impl View for ScrollDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        scroll_view(self.offset.clone())
    }
}

/// Row `(id, y)` pairs from `focusables()`, filtered to the rows (height
/// `ROW_H`) — excludes the ScrollView itself (height `VIEWPORT_H`) — sorted by
/// vertical position. At offset 0 the rows sit at y = 0, 60, 120, 180.
fn rows_top_to_bottom(app: &HeadlessApp) -> Vec<(ElementId, f32)> {
    let mut rows: Vec<(ElementId, f32)> = app
        .focusables()
        .into_iter()
        .filter(|(_, b)| (b.size.height - ROW_H).abs() < 0.5)
        .map(|(id, b)| (id, b.origin.y))
        .collect();
    rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    rows
}

#[test]
fn focusing_offscreen_child_scrolls_it_into_view() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, 200, 1.0).expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    let mut view = ScrollDemo {
        offset: offset.clone(),
    };

    // First render at offset 0 to register the focusables.
    let _ = app.render_view(&mut view).expect("first render");
    let rows = rows_top_to_bottom(&app);
    assert_eq!(rows.len(), N_ROWS, "all four rows are focusable");
    let last_row = rows[N_ROWS - 1].0; // bottom row, fully off-screen (y = 180).
    let first_row = rows[0].0; // top row, visible at y = 0.
    assert_eq!(offset.get(), 0.0);

    // Focus the bottom row → next render reveals it.
    assert!(app.set_focus(last_row), "bottom row is focusable");
    let _ = app.render_view(&mut view).expect("render after focusing bottom");
    // Bottom row bottom = 240, viewport 100 → reveal offset = 140 (== max).
    assert!(
        (offset.get() - 140.0).abs() < 0.5,
        "focusing the bottom row scrolls it fully into view (offset {})",
        offset.get()
    );

    // Re-render with focus unchanged: reveal already reacted, so the offset must
    // not move again (it does not fight a settled position).
    let _ = app.render_view(&mut view).expect("idempotent render");
    assert!(
        (offset.get() - 140.0).abs() < 0.5,
        "no further scroll while focus is unchanged (offset {})",
        offset.get()
    );

    // Focus the top row (now scrolled off the top) → reveal scrolls back up.
    assert!(app.set_focus(first_row), "top row is focusable");
    let _ = app.render_view(&mut view).expect("render after focusing top");
    assert!(
        offset.get().abs() < 0.5,
        "focusing the top row scrolls back to the top (offset {})",
        offset.get()
    );
}

#[test]
fn focusing_already_visible_child_does_not_scroll() {
    let mut app = HeadlessApp::with_scale_factor(VIEWPORT_W as u32, 200, 1.0).expect("app");
    let offset = Signal::new(app.runtime(), 0.0);
    let mut view = ScrollDemo {
        offset: offset.clone(),
    };

    let _ = app.render_view(&mut view).expect("first render");
    let rows = rows_top_to_bottom(&app);
    let top_row = rows[0].0; // y = 0, already fully visible.

    assert!(app.set_focus(top_row));
    let _ = app.render_view(&mut view).expect("render after focusing visible row");
    assert_eq!(
        offset.get(),
        0.0,
        "focusing an already-visible child leaves the offset at the top"
    );
}
