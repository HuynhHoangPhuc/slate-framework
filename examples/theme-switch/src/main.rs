//! theme-switch — ambient design tokens + live light/dark switching.
//!
//! The whole UI is styled from semantic theme tokens read ambiently via
//! `theme()` — note that `swatch()` and `panel()` are free functions that read
//! the theme without any `Theme` being passed in (no prop-drilling).
//!
//! Click the button to flip the app's `ThemeMode` signal. The flip re-renders
//! the whole view with the other palette (Strategy A).
//!
//! Run: `cargo run -p theme-switch`

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, JustifyContent,
    Text, ThemeMode, View, WindowOptions, theme,
};

struct ThemeSwitchView {
    /// App-global light/dark selector, obtained from `AppContext::theme_mode`.
    mode: Signal<ThemeMode>,
}

/// A labelled colour chip. Reads `theme()` ambiently for its border + spacing —
/// nothing theme-related is passed in.
fn swatch(label: &'static str, color: Color) -> Div {
    let t = theme();
    Div::new()
        .style(|s| {
            s.flex_direction(FlexDirection::Column)
                .align_items(AlignItems::Center)
                .gap(t.spacing.xs)
        })
        .child(
            Div::new()
                .background(color)
                .corner_radius(t.radius.sm)
                .style(|s| s.width(56.0).height(40.0)),
        )
        .child(
            Text::new(label)
                .font_size(t.typography.caption.size)
                .color(t.muted.into()),
        )
}

/// A surface panel that lays out the swatches. Also reads `theme()` ambiently.
fn palette_panel() -> Div {
    let t = theme();
    Div::new()
        .background(t.surface)
        .corner_radius(t.radius.lg)
        .style(|s| {
            s.flex_direction(FlexDirection::Row)
                .gap(t.spacing.lg)
                .padding_all(t.spacing.lg)
        })
        .child(swatch("accent", t.accent))
        .child(swatch("danger", t.danger))
        .child(swatch("border", t.border))
        .child(swatch("muted", t.muted))
}

fn toggle(mode: &Signal<ThemeMode>) {
    mode.update(|m| {
        *m = match *m {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        }
    });
}

impl View for ThemeSwitchView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let t = theme();
        let mode = self.mode.clone();
        let label = match self.mode.get() {
            ThemeMode::Light => "Switch to Dark",
            ThemeMode::Dark => "Switch to Light",
        };

        Div::new()
            .background(t.bg)
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(t.spacing.xl)
                    .padding_all(t.spacing.xl)
                    .flex_grow(1.0)
            })
            .child(
                Text::new("Theme Tokens")
                    .font_size(t.typography.heading.size)
                    .color(t.fg.into()),
            )
            .child(palette_panel())
            .child(
                Div::new()
                    .background(t.accent)
                    .corner_radius(t.radius.md)
                    .style(|s| {
                        s.padding_all(t.spacing.md)
                            .align_items(AlignItems::Center)
                    })
                    .on_click(move |_, _| toggle(&mode))
                    .child(Text::new(label).font_size(t.typography.body.size).color(t.bg.into())),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · theme-switch".into(),
        size: (420, 360),
        min_size: Some((360, 300)),
        resizable: true,
        ..Default::default()
    })
    .run(|cx: &AppContext| {
        let mode = cx
            .theme_mode()
            .expect("theme_mode available inside App::run");
        ThemeSwitchView { mode }
    });
}
