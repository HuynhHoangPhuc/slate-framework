//! hello-element — Phase 7 demo.
//!
//! Demonstrates the Element-based UI framework with nested Div + Text elements,
//! Flexbox layout, and the three-phase element lifecycle.
//!
//! Run: `cargo run -p hello-element`

use slate_framework::{
    AlignItems, AnyElement, App, Color, Div, FlexDirection, IntoAny, JustifyContent, Text, View,
    WindowOptions,
};

/// Simple view demonstrating nested elements with layout.
struct HelloView;

impl View for HelloView {
    fn render(&mut self) -> AnyElement {
        // Root container — dark background, centered column layout
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
            // Title
            .child(
                Text::new("Hello, Slate!")
                    .font_size(32.0)
                    .color(Color::WHITE.into()),
            )
            // Styled container — looks like a button
            .child(
                Div::new()
                    .background(Color::from_hex("#3b82f6").unwrap_or(Color::BLUE).into())
                    .corner_radius(8.0)
                    .style(|s| s.padding_all(12.0))
                    .child(
                        Text::new("Styled container")
                            .font_size(16.0)
                            .color(Color::WHITE.into()),
                    ),
            )
            // Subtitle
            .child(
                Text::new("Phase 3: Layout + Elements working!")
                    .font_size(14.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            // Info row
            .child(
                Div::new()
                    .style(|s| s.flex_direction(FlexDirection::Row).gap(8.0))
                    .child(
                        Text::new("Taffy layout")
                            .font_size(12.0)
                            .color(Color::from_hex("#6b7280").unwrap_or(Color::WHITE).into()),
                    )
                    .child(
                        Text::new("•")
                            .font_size(12.0)
                            .color(Color::from_hex("#6b7280").unwrap_or(Color::WHITE).into()),
                    )
                    .child(
                        Text::new("smol async")
                            .font_size(12.0)
                            .color(Color::from_hex("#6b7280").unwrap_or(Color::WHITE).into()),
                    )
                    .child(
                        Text::new("•")
                            .font_size(12.0)
                            .color(Color::from_hex("#6b7280").unwrap_or(Color::WHITE).into()),
                    )
                    .child(
                        Text::new("GPU rendering")
                            .font_size(12.0)
                            .color(Color::from_hex("#6b7280").unwrap_or(Color::WHITE).into()),
                    ),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · hello-element".into(),
        size: (600, 400),
        min_size: Some((320, 240)),
        resizable: true,
    })
    .run(|| HelloView);
}
