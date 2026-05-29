//! 100 reactive counter cards — exercises signal-notify dispatch and the
//! reactive observer scheduler.
//!
//! Each card binds to its own `Signal<i32>` via `Text::new_reactive`, so a
//! bench iter that bumps every signal forces 100 observer dispatches per
//! frame. Counter-card style is lifted from `examples/reactive-counter` but
//! stripped of focus / IME / key handling.

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, Color, Div, FlexDirection, HeadlessApp, IntoAny, JustifyContent, Text,
    View,
};

const CARDS: usize = 100;

pub struct CounterGridView {
    counts: Vec<Signal<i32>>,
}

impl CounterGridView {
    /// Bump every counter — drives `SIGNAL_NOTIFY_COUNT += CARDS`.
    pub fn bump_all(&self) {
        for c in &self.counts {
            c.update(|n| *n += 1);
        }
    }
}

fn card(label: String, counter: Signal<i32>) -> Div {
    let c_text = counter.clone();
    Div::new()
        .background(Color::from_hex("#3b82f6").unwrap_or(Color::BLUE))
        .corner_radius(6.0)
        .style(|s| {
            s.flex_direction(FlexDirection::Column)
                .align_items(AlignItems::Center)
                .justify_content(JustifyContent::Center)
                .gap(4.0)
                .padding_all(8.0)
                .width(72.0)
                .height(72.0)
        })
        .child(Text::new(label).font_size(10.0).color(Color::WHITE.into()))
        .child(
            Text::new_reactive(move || c_text.get().to_string())
                .font_size(18.0)
                .color(Color::WHITE.into()),
        )
}

impl View for CounterGridView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        // 10×10 grid.
        let mut rows = Div::new().style(|s| {
            s.flex_direction(FlexDirection::Column)
                .align_items(AlignItems::Center)
                .gap(8.0)
        });
        for r in 0..10 {
            let mut row = Div::new().style(|s| s.flex_direction(FlexDirection::Row).gap(8.0));
            for c in 0..10 {
                let idx = r * 10 + c;
                row = row.child(card(format!("#{idx}"), self.counts[idx].clone()));
            }
            rows = rows.child(row);
        }

        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK))
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .padding_all(12.0)
                    .flex_grow(1.0)
            })
            .child(rows)
            .into_any()
    }
}

pub fn build(app: &HeadlessApp) -> CounterGridView {
    let rt = app.runtime();
    let counts = (0..CARDS).map(|_| Signal::new(rt.clone(), 0i32)).collect();
    CounterGridView { counts }
}
