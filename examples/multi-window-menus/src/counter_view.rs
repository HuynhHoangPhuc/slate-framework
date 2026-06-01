//! The per-window counter view: a title, a live count, and a hint line.

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, Color, Div, FlexDirection, IntoAny, JustifyContent, Text, View,
};

/// A single window's content: shows `title` and its own `count` signal.
pub struct CounterView {
    /// Window title shown above the counter.
    pub title: String,
    /// The counter this window's menu drives.
    pub count: Signal<i32>,
}

impl View for CounterView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let count = self.count.clone();
        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK))
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(12.0)
                    .padding_all(24.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new(self.title.clone())
                    .font_size(20.0)
                    .color(Color::WHITE.into()),
            )
            .child(
                Text::new_reactive(move || format!("{}", count.get()))
                    .font_size(48.0)
                    .color(Color::WHITE.into()),
            )
            .child(
                Text::new(
                    "This window's menu ▸ Increment (⌘I) / Reset   ·   Window ▸ New Window (⌘N)",
                )
                .font_size(11.0)
                .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}
