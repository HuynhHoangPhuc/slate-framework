//! Large virtualized scroll list — exercises ScrollView windowing under a
//! huge row count.
//!
//! A 100k-row list scrolled forward one window-step per frame. The whole point
//! of virtualization is that per-frame work (layout nodes, prepaint, paint
//! commands) tracks the *visible* window — a few dozen rows — not the 100k
//! total. The bench's `paint_cmds` counter is the headline assertion: it should
//! stay in the tens regardless of `ROW_COUNT`.

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, Color, Div, FlexDirection, HeadlessApp, IntoAny, ScrollView, Text, View,
};

/// Total rows in the virtual list. Deliberately far larger than anything that
/// could fit on screen, to prove the per-frame cost is window-bounded.
const ROW_COUNT: usize = 100_000;
/// Uniform row height (logical px).
const ROW_HEIGHT: f32 = 28.0;
/// Scroll viewport height (logical px) — a tall panel within the bench window.
const VIEWPORT_H: f32 = 720.0;
/// Viewport width (logical px).
const VIEWPORT_W: f32 = 960.0;
/// Rows advanced per frame so the visible window slides every iteration.
const ROWS_PER_FRAME: f32 = 7.0;

pub struct LargeScrollView {
    offset: Signal<f32>,
}

impl LargeScrollView {
    /// Scroll the window forward by a few rows, wrapping at the bottom — keeps
    /// the windowing + clip + rebuild path engaged on every bench iter.
    pub fn advance(&self) {
        let max_offset = (ROW_COUNT as f32 * ROW_HEIGHT - VIEWPORT_H).max(0.0);
        let step = ROWS_PER_FRAME * ROW_HEIGHT;
        self.offset.update(|o| {
            let next = *o + step;
            *o = if next > max_offset { 0.0 } else { next };
        });
    }
}

/// One dashboard-style row: index label + a value cell, on an alternating fill.
fn row(i: usize) -> Div {
    let fill = if i.is_multiple_of(2) {
        Color::from_hex("#15151f").unwrap_or(Color::BLACK)
    } else {
        Color::from_hex("#1b1b27").unwrap_or(Color::BLACK)
    };
    Div::new()
        .background(fill)
        .style(|s| {
            s.flex_direction(FlexDirection::Row)
                .align_items(AlignItems::Center)
                .gap(16.0)
                .width(VIEWPORT_W)
                .height(ROW_HEIGHT)
                .padding_all(4.0)
        })
        .child(Text::new(format!("row {i:>6}")).font_size(13.0).color(Color::WHITE.into()))
        .child(Text::new(format!("value {}", i * 3)).font_size(13.0).color(Color::WHITE.into()))
}

impl View for LargeScrollView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new()
            .background(Color::from_hex("#0d0d14").unwrap_or(Color::BLACK))
            .style(|s| s.flex_direction(FlexDirection::Column).flex_grow(1.0))
            .child(
                ScrollView::new()
                    .offset(self.offset.clone())
                    .style(|s| s.width(VIEWPORT_W).height(VIEWPORT_H).column())
                    .virtualized(ROW_COUNT, ROW_HEIGHT, row),
            )
            .into_any()
    }
}

pub fn build(app: &HeadlessApp) -> LargeScrollView {
    let offset = Signal::new(app.runtime(), 0.0);
    LargeScrollView { offset }
}
