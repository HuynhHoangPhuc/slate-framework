//! reactive-counter — Phase 5 demo.
//!
//! Demonstrates reactive UI with Signal-driven Text that updates automatically
//! when a background task increments the counter.
//!
//! Run: `cargo run -p reactive-counter`

use std::time::Duration;

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, JustifyContent,
    Text, Timer, View, WindowOptions,
};

struct CounterView {
    count: Signal<u32>,
}

impl View for CounterView {
    fn render(&mut self) -> AnyElement {
        let c = self.count.clone();

        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into())
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(16.0)
                    .padding_all(32.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new("Reactive Counter")
                    .font_size(24.0)
                    .color(Color::WHITE.into()),
            )
            .child(
                Div::new()
                    .background(Color::from_hex("#3b82f6").unwrap_or(Color::BLUE).into())
                    .corner_radius(8.0)
                    .style(|s| s.padding_all(16.0))
                    .child(
                        Text::new_reactive(move || format!("Count: {}", c.get()))
                            .font_size(32.0)
                            .color(Color::WHITE.into()),
                    ),
            )
            .child(
                Text::new("Increments every second via background task")
                    .font_size(14.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · reactive-counter".into(),
        size: (400, 300),
        min_size: Some((320, 240)),
        resizable: true,
    })
    .run(|cx: &AppContext| {
        let count = Signal::new(cx.runtime(), 0u32);

        let bg = cx.background_executor();
        let counter = count.clone();
        bg.spawn(async move {
            loop {
                Timer::after(Duration::from_secs(1)).await;
                counter.update(|n| *n += 1);
                log::info!("Counter incremented to {}", counter.get());
            }
        })
        .detach();

        CounterView { count }
    });
}
