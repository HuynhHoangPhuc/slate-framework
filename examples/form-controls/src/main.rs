//! form-controls — the P5a adoption-completeness control set on one screen.
//!
//! Exercises every control in the foundation set — Button, IconButton, Checkbox,
//! Switch, Slider, and a theme-wrapped TextField — all styled from ambient theme
//! tokens and operable by keyboard (Tab to move, Enter/Space to activate, arrows
//! to adjust the slider). The top-right button flips light/dark to prove the
//! controls re-theme through `theme()` (Strategy A).
//!
//! Run: `cargo run -p form-controls`

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Button, Checkbox, Div, FlexDirection, IconButton,
    IntoAny, Slider, Switch, Text, TextField, ThemeMode, View, WindowOptions, theme,
};

/// All caller-owned control state for the demo, created once in `App::run`.
struct FormControlsView {
    mode: Signal<ThemeMode>,
    notifications: Signal<bool>,
    compact: Signal<bool>,
    volume: Signal<f32>,
    name: Signal<String>,
}

/// A titled surface card that groups related controls. Reads `theme()` ambiently.
fn card(title: &'static str, body: Div) -> Div {
    let t = theme();
    Div::new()
        .background(t.surface)
        .corner_radius(t.radius.lg)
        .style(|s| {
            s.flex_direction(FlexDirection::Column)
                .gap(t.spacing.md)
                .padding_all(t.spacing.lg)
        })
        .child(
            Text::new(title)
                .font_size(t.typography.caption.size)
                .color(t.muted.into()),
        )
        .child(body)
}

fn toggle_mode(mode: &Signal<ThemeMode>) {
    mode.update(|m| {
        *m = match *m {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        }
    });
}

impl View for FormControlsView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let t = theme();

        // --- Header: title + light/dark toggle button ---
        let mode = self.mode.clone();
        let theme_label = match self.mode.get() {
            ThemeMode::Light => "Dark mode",
            ThemeMode::Dark => "Light mode",
        };
        let header = Div::new()
            .style(|s| {
                s.flex_direction(FlexDirection::Row)
                    .align_items(AlignItems::Center)
                    .gap(t.spacing.lg)
            })
            .child(
                Text::new("Form Controls")
                    .font_size(t.typography.heading.size)
                    .color(t.fg.into()),
            )
            .child(Button::new(theme_label).on_click(move |_| toggle_mode(&mode)));

        // --- Buttons card ---
        let vol_reset = self.volume.clone();
        let buttons = Div::new()
            .style(|s| {
                s.flex_direction(FlexDirection::Row)
                    .align_items(AlignItems::Center)
                    .gap(t.spacing.md)
            })
            .child(Button::new("Save").on_click(|_| log::info!("Save clicked")))
            .child(Button::new("Disabled").disabled(true))
            .child(Button::new("Reset volume").on_click(move |_| vol_reset.set(0.0)))
            .child(IconButton::new(
                Text::new("✕")
                    .font_size(t.typography.body.size)
                    .color(t.bg.into())
                    .presentational(),
                "Dismiss",
            ));

        // --- Toggles card: checkbox + switch ---
        let toggles = Div::new()
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .gap(t.spacing.md)
            })
            .child(Checkbox::new(self.notifications.clone(), "Enable notifications"))
            .child(Switch::new(self.compact.clone(), "Compact layout"));

        // --- Slider card with a live value readout ---
        let vol = self.volume.clone();
        let slider = Div::new()
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .gap(t.spacing.sm)
            })
            .child(Text::new_reactive(move || format!("Volume: {}", vol.get() as i64)).color(t.fg.into()))
            .child(Slider::new(self.volume.clone(), 0.0, 100.0).step(5.0).width(220.0).label("Volume"));

        // --- Themed text input ---
        let field = TextField::new(self.name.clone()).themed();

        Div::new()
            .background(t.bg)
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .gap(t.spacing.lg)
                    .padding_all(t.spacing.xl)
                    .flex_grow(1.0)
            })
            .child(header)
            .child(card("Buttons", buttons))
            .child(card("Toggles", toggles))
            .child(card("Slider", slider))
            .child(card("Text input", Div::new().child(field)))
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · form-controls".into(),
        size: (520, 640),
        min_size: Some((420, 520)),
        resizable: true,
        ..Default::default()
    })
    .run(|cx: &AppContext| {
        let rt = cx.runtime();
        let mode = cx
            .theme_mode()
            .expect("theme_mode available inside App::run");
        FormControlsView {
            mode,
            notifications: Signal::new(rt.clone(), true),
            compact: Signal::new(rt.clone(), false),
            volume: Signal::new(rt.clone(), 40.0),
            name: Signal::new(rt, String::new()),
        }
    });
}
