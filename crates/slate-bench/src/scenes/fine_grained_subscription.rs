//! 100 reactive counter cards — exercises Strategy A whole-view rebuild
//! under a 1-of-100-dirty workload.
//!
//! Visual layout identical to `reactive_counter_100`; only the mutation
//! method differs: this scene exposes `bump_one(index)` (single signal
//! mutation) instead of `bump_all` (all 100 mutated). Under Strategy A the
//! single mutation still triggers a whole-window observer notification and
//! a full `View::render` rebuild — exactly the workload the A1
//! fine-grained-reactivity verdict needs to measure.

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
    /// Bump a single counter. Under Strategy A this still notifies the
    /// whole-window observer and triggers a full `View::render` rebuild.
    pub fn bump_one(&self, index: usize) {
        self.counts[index].update(|n| *n += 1);
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
