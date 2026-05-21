//! click-counter — Phase 5a mouse events demo.
//!
//! Demonstrates on_click handler with reactive counter that increments on click.
//!
//! Run: `cargo run -p click-counter`

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, JustifyContent,
    Text, View, WindowOptions,
};

struct ClickCounterView {
    count: Signal<u32>,
}

impl View for ClickCounterView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let c = self.count.clone();

        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK))
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(16.0)
                    .padding_all(32.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new("Click Counter")
                    .font_size(24.0)
                    .color(Color::WHITE.into()),
            )
            .child({
                let count = self.count.clone();
                Div::new()
                    .background(Color::from_hex("#3b82f6").unwrap_or(Color::BLUE))
                    .corner_radius(8.0)
                    .style(|s| s.padding_all(16.0))
                    .on_click(move |_, _| {
                        count.update(|n| *n += 1);
                        log::info!("Button clicked! Count: {}", count.get());
                    })
                    .child(
                        Text::new_reactive(move || format!("Count: {}", c.get()))
                            .font_size(32.0)
                            .color(Color::WHITE.into()),
                    )
            })
            .child(
                Text::new("Click the blue button above")
                    .font_size(14.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · click-counter".into(),
        size: (400, 300),
        min_size: Some((320, 240)),
        resizable: true,
        ..Default::default()
    })
    .run(|cx: &AppContext| {
        let count = Signal::new(cx.runtime(), 0u32);
        ClickCounterView { count }
    });
}
