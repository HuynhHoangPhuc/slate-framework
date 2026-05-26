//! IME textfield — single-line `TextField` with reactive value binding.
//!
//! Mirrors `examples/ime-textfield` but stripped to the rendering core. The
//! bench iter mutates the bound `Signal<String>` to simulate keystroke
//! traffic, so every frame walks shaping + caret + paint.

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, Color, Div, FlexDirection, HeadlessApp, IntoAny,
    JustifyContent, Text, TextField, TextFieldStyle, View,
};

const SEED: &str = "Hello مرحبا world";

pub struct ImeFieldView {
    value: Signal<String>,
    tick: u64,
}

impl ImeFieldView {
    /// Simulate a keystroke — append a counter so the field re-shapes per
    /// iter.
    pub fn type_char(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.value.set(format!("{SEED} {}", self.tick));
    }
}

impl View for ImeFieldView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let echo = self.value.clone();

        let field_style = TextFieldStyle {
            font_size: 18.0,
            color: Color::WHITE.into(),
            background: Some([0.15, 0.15, 0.18, 1.0]),
            caret_color: Color::WHITE.into(),
            preedit_selection_color: [0.4, 0.6, 1.0, 0.3],
            width: 480.0,
        };

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
            .child(TextField::new(self.value.clone()).style(field_style))
            .child(
                Text::new_reactive(move || echo.get())
                    .font_size(12.0)
                    .color(Color::WHITE.into()),
            )
            .into_any()
    }
}

pub fn build(app: &HeadlessApp) -> ImeFieldView {
    let value = Signal::new(app.runtime(), SEED.to_string());
    ImeFieldView { value, tick: 0 }
}
