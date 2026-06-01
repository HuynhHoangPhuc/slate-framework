//! overlay-widgets — the P5c overlay-based widget set on one screen.
//!
//! Exercises all three overlay widgets that ride the P3 overlay layer:
//! - **Select** — a dropdown bound to a `Signal<usize>`; opens a flip-aware,
//!   keyboard-navigable option list (focus moves in on open, arrows rove, Enter
//!   chooses, Esc / click-away dismiss) without dimming the app.
//! - **Tooltip** — wraps a button; rest the pointer on it to reveal a delayed
//!   label bubble that hides on leave.
//! - **ContextMenu** — right-click the bordered area to open an in-canvas menu
//!   at the pointer; arrow-navigable, dismiss on choose / Esc / outside-click.
//!
//! The top-right button flips light/dark to prove every widget re-themes through
//! `theme()` (Strategy A). All widget state is caller-owned `Signal`s on the View.
//!
//! Run: `cargo run -p overlay-widgets`

use slate_framework::reactive::Signal;
use slate_framework::{
    AnyElement, App, AppContext, Bounds, Button, ContextMenu, Div, IntoAny, Select, Text, ThemeMode,
    Tooltip, View, WindowOptions, theme,
};

use std::time::Instant;

/// All caller-owned widget state, created once in `App::run`.
struct OverlayWidgetsView {
    mode: Signal<ThemeMode>,
    // Select: chosen index + open flag + tracked trigger rect + list scroll.
    priority: Signal<usize>,
    select_open: Signal<bool>,
    select_anchor: Signal<Bounds>,
    select_scroll: Signal<f32>,
    // ContextMenu: open flag + pointer anchor.
    ctx_open: Signal<bool>,
    ctx_anchor: Signal<Bounds>,
    // Tooltip: hover-start instant.
    tip_hover: Signal<Option<Instant>>,
    // Last action label, updated by menu selections (proves activation fires).
    last_action: Signal<String>,
}

fn toggle_mode(mode: &Signal<ThemeMode>) {
    mode.update(|m| {
        *m = match *m {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        }
    });
}

/// A titled surface card grouping a demo widget.
fn card(title: &str, body: impl IntoAny) -> Div {
    let t = theme();
    Div::new()
        .background(t.surface)
        .corner_radius(t.radius.lg)
        .border(t.border, 1.0)
        .style(|s| s.column().gap(t.spacing.md).padding_all(t.spacing.lg))
        .child(
            Text::new(title)
                .font_size(t.typography.caption.size)
                .color(t.muted.into()),
        )
        .child_any(body.into_any())
}

impl OverlayWidgetsView {
    fn header(&self) -> Div {
        let t = theme();
        let mode = self.mode.clone();
        let label = match self.mode.get() {
            ThemeMode::Light => "Dark mode",
            ThemeMode::Dark => "Light mode",
        };
        Div::new()
            .style(|s| s.row().gap(t.spacing.md))
            .child(
                Text::new("Overlay widgets")
                    .font_size(t.typography.heading.size)
                    .color(t.fg.into()),
            )
            .child(Button::new(label).on_click(move |_| toggle_mode(&mode)))
    }

    fn select_card(&self) -> Div {
        let select = Select::new(
            self.priority.clone(),
            self.select_open.clone(),
            self.select_anchor.clone(),
        )
        .options(["Lowest", "Low", "Normal", "High", "Highest", "Critical"])
        .scroll(self.select_scroll.clone());
        card("Select — task priority", select)
    }

    fn tooltip_card(&self) -> Div {
        let tip = Tooltip::new(self.tip_hover.clone(), "Permanently deletes the item")
            .child(Button::new("Delete"));
        card("Tooltip — hover the button", tip)
    }

    fn context_card(&self) -> Div {
        let t = theme();
        let last = self.last_action.clone();
        let target = Div::new()
            .background(t.bg)
            .corner_radius(t.radius.md)
            .border(t.border, 1.0)
            .style(|s| s.height(80.0).padding_all(t.spacing.md))
            .child(
                Text::new("Right-click anywhere in here")
                    .font_size(t.typography.body.size)
                    .color(t.muted.into()),
            );
        let items = ["Rename", "Duplicate", "Delete"];
        let ctx = ContextMenu::new(self.ctx_open.clone(), self.ctx_anchor.clone())
            .items(items)
            .on_select(move |i| last.set(format!("Context action: {}", items[i])))
            .child(target);
        card("ContextMenu — in-canvas right-click", ctx)
    }
}

impl View for OverlayWidgetsView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let t = theme();
        Div::new()
            .background(t.bg)
            .style(|s| s.column().gap(t.spacing.lg).padding_all(t.spacing.xl).flex_grow(1.0))
            .child(self.header())
            .child(self.select_card())
            .child(self.tooltip_card())
            .child(self.context_card())
            .child(
                Text::new(self.last_action.get())
                    .font_size(t.typography.caption.size)
                    .color(t.muted.into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · overlay-widgets".into(),
        size: (520, 620),
        min_size: Some((420, 480)),
        resizable: true,
        ..Default::default()
    })
    .run(|cx: &AppContext| {
        let rt = cx.runtime();
        let mode = cx.theme_mode().expect("theme_mode available inside App::run");
        OverlayWidgetsView {
            mode,
            priority: Signal::new(rt.clone(), 2),
            select_open: Signal::new(rt.clone(), false),
            select_anchor: Signal::new(rt.clone(), Bounds::ZERO),
            select_scroll: Signal::new(rt.clone(), 0.0),
            ctx_open: Signal::new(rt.clone(), false),
            ctx_anchor: Signal::new(rt.clone(), Bounds::ZERO),
            tip_hover: Signal::new(rt.clone(), None),
            last_action: Signal::new(rt, "No action yet".to_string()),
        }
    });
}
