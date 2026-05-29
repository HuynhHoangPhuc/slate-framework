//! Smoke tests for the per-phase redraw counters
//! (`view_render_count`, `view_render_ns`, `compute_layout_ns`, `paint_ns`,
//! `present_ns`).
//!
//! Verifies reset semantics + that a single `HeadlessApp::render_view` call
//! advances all 5 counters (proves the headless-path instrumentation in
//! `headless/render.rs` fires — this is the code path bench harnesses use).

#![cfg(feature = "profiling")]

use std::sync::{Mutex, MutexGuard, OnceLock};

use slate_framework::profiling;
use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, Color, Div, FlexDirection, HeadlessApp, IntoAny, JustifyContent,
    RenderCx, Text, View,
};

fn lock() -> MutexGuard<'static, ()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    let m = M.get_or_init(|| Mutex::new(()));
    m.lock().unwrap_or_else(|p| p.into_inner())
}

struct TinyCounterView {
    count: Signal<i32>,
}

impl View for TinyCounterView {
    fn render(&mut self, _cx: &mut RenderCx) -> AnyElement {
        let c = self.count.clone();
        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK))
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .padding_all(8.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new_reactive(move || c.get().to_string())
                    .font_size(14.0)
                    .color(Color::WHITE.into()),
            )
            .into_any()
    }
}

#[test]
fn reset_zeroes_all_redraw_counters() {
    let _g = lock();
    profiling::reset_counters();

    assert_eq!(profiling::view_render_count(), 0);
    assert_eq!(profiling::view_render_ns(), 0);
    assert_eq!(profiling::compute_layout_ns(), 0);
    assert_eq!(profiling::paint_ns(), 0);
    assert_eq!(profiling::present_ns(), 0);
}

#[test]
fn headless_render_view_advances_all_redraw_counters() {
    let _g = lock();
    profiling::reset_counters();

    let mut app = HeadlessApp::new(128, 128).expect("headless app");
    let count = Signal::new(app.runtime(), 0i32);
    let mut view = TinyCounterView { count };

    let _img = app.render_view(&mut view).expect("render_view");

    assert_eq!(
        profiling::view_render_count(),
        1,
        "view_render_count should be exactly 1 after one render_view",
    );
    assert!(
        profiling::view_render_ns() > 0,
        "view_render_ns should be > 0 after a render",
    );
    assert!(
        profiling::compute_layout_ns() > 0,
        "compute_layout_ns should be > 0 after a render",
    );
    assert!(
        profiling::paint_ns() > 0,
        "paint_ns should be > 0 after a render",
    );
    assert!(
        profiling::present_ns() > 0,
        "present_ns should be > 0 after a render",
    );
}
