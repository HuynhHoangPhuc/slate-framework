//! overlay-popover — anchored Overlay demo (P3 foundation).
//!
//! Click the button to toggle a popover that:
//! - is **anchored** below the button via the pure anchor solver,
//! - paints **on top** of the background list (high-depth scene layer),
//! - **flips** above the button automatically if the window is short enough
//!   that there's no room below (drag the bottom edge up to see it).
//!
//! The popover's `open` flag is a caller-owned `Signal<bool>` held by the view,
//! so toggling it re-renders under the render observer (Strategy-A rebuild).
//!
//! Run: `cargo run -p overlay-popover`

use slate_framework::reactive::Signal;
use slate_framework::{
    AnyElement, App, AppContext, Bounds, Color, Div, IntoAny, Overlay, Placement, Text, View,
    WindowOptions,
};

/// The button's fixed rect (outer padding 40 + explicit button size), reused as
/// the popover anchor. Kept in one place so the marker and the anchor agree.
fn button_bounds() -> Bounds {
    Bounds::from_origin_size(40.0, 40.0, 220.0, 48.0)
}

struct PopoverDemo {
    open: Signal<bool>,
}

impl PopoverDemo {
    fn background_list() -> Div {
        let mut list = Div::new().style(|s| s.column().gap(8.0).padding_all(40.0).flex_grow(1.0));
        // A toggle button at the top, then filler rows the popover will cover.
        for i in 0..8 {
            list = list.child(
                Div::new()
                    .background(Color::from_hex("#313244").unwrap_or(Color::BLACK))
                    .corner_radius(6.0)
                    .style(|s| s.height(28.0).width(380.0))
                    .child(
                        Text::new(format!("background row {i}"))
                            .font_size(13.0)
                            .color(Color::from_hex("#9399b2").unwrap_or(Color::WHITE).into()),
                    ),
            );
        }
        list
    }

    fn button(&self) -> Div {
        let open = self.open.clone();
        let label = self.open.clone();
        Div::new()
            .background(Color::from_hex("#89b4fa").unwrap_or(Color::BLUE))
            .corner_radius(8.0)
            .style(|s| s.width(220.0).height(48.0).padding_all(12.0))
            .on_click(move |_, _| open.update(|o| *o = !*o))
            .child(
                Text::new_reactive(move || {
                    if label.get() {
                        "Hide popover ▲".to_string()
                    } else {
                        "Show popover ▼".to_string()
                    }
                })
                .font_size(16.0)
                .color(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into()),
            )
    }

    fn popover() -> Overlay {
        Overlay::new()
            .anchor(button_bounds())
            .placement(Placement::bottom())
            .child(
                Div::new()
                    .background(Color::from_hex("#cdd6f4").unwrap_or(Color::WHITE))
                    .corner_radius(10.0)
                    .style(|s| s.width(260.0).column().gap(6.0).padding_all(14.0))
                    .child(Text::new("Popover").font_size(18.0).color(
                        Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into(),
                    ))
                    .child(Text::new("Anchored below the button.").font_size(13.0).color(
                        Color::from_hex("#45475a").unwrap_or(Color::BLACK).into(),
                    ))
                    .child(
                        Text::new("Drawn above the list; flips up at the edge.")
                            .font_size(13.0)
                            .color(Color::from_hex("#45475a").unwrap_or(Color::BLACK).into()),
                    ),
            )
    }
}

impl View for PopoverDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let root = Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK))
            .style(|s| s.column().flex_grow(1.0))
            // The button is the first child inside 40px padding, so it lands at
            // exactly `button_bounds()` — the rect the popover anchors to.
            .child(
                Div::new()
                    .style(|s| s.padding_all(40.0))
                    .child(self.button()),
            )
            .child(Self::background_list());

        // Reading `open` under the render observer subscribes the view, so the
        // toggle rebuilds and shows/hides the overlay.
        if self.open.get() {
            root.child(Self::popover()).into_any()
        } else {
            root.into_any()
        }
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · overlay-popover".into(),
        size: (480, 420),
        min_size: Some((360, 200)),
        resizable: true,
        ..Default::default()
    })
    .run(|cx: &AppContext| PopoverDemo {
        open: Signal::new(cx.runtime(), true),
    });
}
