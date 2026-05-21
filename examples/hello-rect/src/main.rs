//! hello-rect — Anchor demo using App::new.
//!
//! Opens a native window and renders a single antialiased rounded rectangle
//! using the framework's element system with automatic device-lost recovery.
//!
//! Run: `cargo run -p hello-rect`

use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, JustifyContent,
    View, WindowOptions,
};

struct HelloRectView;

impl View for HelloRectView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        // Outer container: flex column, centered
        Div::new()
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .flex_grow(1.0)
            })
            .child(
                // Inner rect: sky blue (#66ccff), 400x200 logical pixels, corner radius 24
                Div::new()
                    .background(Color::from_hex("#66ccff").unwrap_or(Color::BLUE))
                    .corner_radius(24.0)
                    .style(|s| s.width(400.0).height(200.0)),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · hello-rect".into(),
        size: (800, 600),
        min_size: Some((320, 240)),
        resizable: true,
        ..Default::default()
    })
    .run(|_cx: &AppContext| HelloRectView);
}
