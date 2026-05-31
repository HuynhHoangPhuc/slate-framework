//! overlay-popover — anchored Overlay + modal dialog demo.
//!
//! **Popover** (top button) toggles a non-modal overlay that:
//! - is **anchored** to the button's *live* rect via `Div::track_bounds` +
//!   `Overlay::anchor` (anchor-to-element — no hardcoded coordinates),
//! - paints **on top** of the background list (high-depth scene layer),
//! - **flips** above the button automatically if the window is short enough
//!   that there's no room below (drag the bottom edge up to see it).
//!
//! **Modal dialog** (second button) toggles a `modal(true)` overlay that:
//! - dims the whole window behind it with a **scrim**,
//! - **traps Tab focus** to its two buttons (Tab/Shift+Tab cycle inside only),
//! - **auto-focuses** its first button on open and **restores** the previously
//!   focused element when it closes.
//!
//! The popover's `open` flag, the dialog's `dialog_open` flag, and the button's
//! anchor rect are caller-owned `Signal`s held by the view: toggling a flag
//! re-renders under the render observer (Strategy-A rebuild), and the button
//! reports its painted bounds into `anchor` each frame so the popover follows it.
//!
//! Run: `cargo run -p overlay-popover`

use slate_framework::reactive::Signal;
use slate_framework::{
    AnyElement, App, AppContext, Bounds, Color, Div, IntoAny, Overlay, Placement, Text, View,
    WindowOptions,
};

struct PopoverDemo {
    /// Whether the popover is shown (toggled by the button).
    open: Signal<bool>,
    /// Whether the modal dialog is shown (toggled by the second button).
    dialog_open: Signal<bool>,
    /// The button's live painted rect, written via `Div::track_bounds` and read
    /// back as the popover's anchor — no hardcoded coordinates.
    anchor: Signal<Bounds>,
}

impl PopoverDemo {
    fn background_list() -> Div {
        let mut list = Div::new().style(|s| s.column().gap(8.0).padding_all(40.0).flex_grow(1.0));
        // Filler rows the popover will cover.
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
            // Report the button's live rect so the popover anchors to it.
            .track_bounds(self.anchor.clone())
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

    fn popover(&self) -> Overlay {
        Overlay::new()
            // Anchor to the button's tracked rect (anchor-to-element).
            .anchor(self.anchor.clone())
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

    /// Button that opens the modal dialog.
    fn dialog_button(&self) -> Div {
        let dialog_open = self.dialog_open.clone();
        Div::new()
            .focusable(true)
            .background(Color::from_hex("#a6e3a1").unwrap_or(Color::GREEN))
            .corner_radius(8.0)
            .style(|s| s.width(220.0).height(48.0).padding_all(12.0))
            .on_click(move |_, _| dialog_open.set(true))
            .child(
                Text::new("Open dialog")
                    .font_size(16.0)
                    .color(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into()),
            )
    }

    /// A `modal(true)` dialog: scrim + focus trap. Anchored near window-top-center
    /// and placed below so it sits over the content. Its two buttons are the only
    /// Tab-reachable elements while open; closing restores the prior focus.
    fn dialog(&self) -> Overlay {
        let close = self.dialog_open.clone();
        Overlay::new()
            .modal(true)
            .anchor(Bounds::from_origin_size(140.0, 90.0, 200.0, 0.0))
            .placement(Placement::bottom())
            .child(
                Div::new()
                    .background(Color::from_hex("#cdd6f4").unwrap_or(Color::WHITE))
                    .corner_radius(12.0)
                    .style(|s| s.width(280.0).column().gap(12.0).padding_all(18.0))
                    .child(Text::new("Modal dialog").font_size(18.0).color(
                        Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into(),
                    ))
                    .child(Text::new("Tab cycles between the buttons.").font_size(13.0).color(
                        Color::from_hex("#45475a").unwrap_or(Color::BLACK).into(),
                    ))
                    .child(
                        Div::new()
                            .style(|s| s.row().gap(10.0))
                            .child(
                                Div::new()
                                    .focusable(true)
                                    .background(Color::from_hex("#89b4fa").unwrap_or(Color::BLUE))
                                    .corner_radius(8.0)
                                    .style(|s| s.width(110.0).height(40.0).padding_all(10.0))
                                    .on_click(move |_, _| close.set(false))
                                    .child(Text::new("Close").font_size(14.0).color(
                                        Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into(),
                                    )),
                            )
                            .child(
                                Div::new()
                                    .focusable(true)
                                    .background(Color::from_hex("#f38ba8").unwrap_or(Color::RED))
                                    .corner_radius(8.0)
                                    .style(|s| s.width(110.0).height(40.0).padding_all(10.0))
                                    .child(Text::new("Cancel").font_size(14.0).color(
                                        Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into(),
                                    )),
                            ),
                    ),
            )
    }
}

impl View for PopoverDemo {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let mut root = Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK))
            .style(|s| s.column().flex_grow(1.0))
            .child(
                Div::new()
                    .style(|s| s.row().gap(12.0).padding_all(40.0))
                    .child(self.button())
                    .child(self.dialog_button()),
            )
            .child(Self::background_list());

        // Reading the signals under the render observer subscribes the view, so
        // toggling either rebuilds and shows/hides its overlay. Overlays are laid
        // out absolutely, so adding them as children never disturbs the column.
        if self.open.get() {
            root = root.child(self.popover());
        }
        if self.dialog_open.get() {
            root = root.child(self.dialog());
        }
        root.into_any()
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
        // Start closed: the button paints first and populates `anchor`, so the
        // popover is correctly anchored the moment it opens (no first-frame flash).
        open: Signal::new(cx.runtime(), false),
        dialog_open: Signal::new(cx.runtime(), false),
        anchor: Signal::new(cx.runtime(), Bounds::ZERO),
    });
}
