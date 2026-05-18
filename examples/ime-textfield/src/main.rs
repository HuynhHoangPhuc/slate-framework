//! ime-textfield — Phase 9c demo: single editable TextField with IME composition.
//!
//! - Click the TextField to focus, then type ASCII or switch to a CJK IME.
//! - "You typed:" reactively echoes the bound `Signal<String>`.
//! - Tab moves focus away (currently cycles to self since this is the only
//!   focusable element).
//!
//! Run: `cargo run -p ime-textfield`

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, JustifyContent,
    Text, TextField, TextFieldStyle, View, WindowOptions,
};

struct ImeDemo {
    value: Signal<String>,
}

impl View for ImeDemo {
    fn render(&mut self) -> AnyElement {
        let echo = self.value.clone();

        let field_style = TextFieldStyle {
            font_size: 18.0,
            color: Color::WHITE.into(),
            background: Some([0.15, 0.15, 0.18, 1.0]),
            caret_color: Color::WHITE.into(),
            preedit_selection_color: [0.4, 0.6, 1.0, 0.3],
            width: 400.0,
        };

        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into())
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(20.0)
                    .padding_all(32.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new("Slate IME demo — click the field, then type")
                    .font_size(16.0)
                    .color(Color::WHITE.into()),
            )
            .child(TextField::new(self.value.clone()).style(field_style))
            .child(
                Text::new_reactive(move || {
                    let v = echo.get();
                    if v.is_empty() {
                        "You typed: (empty)".to_string()
                    } else {
                        format!("You typed: {v}")
                    }
                })
                .font_size(14.0)
                .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .child(
                Text::new(
                    "Switch IME — macOS: Ctrl+Space · Windows: Win+Space. \
                     Try Pinyin \"nihao\", Hiragana \"konnichiha\", Hangul \"annyeonghaseyo\".",
                )
                .font_size(12.0)
                .color(Color::from_hex("#64748b").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    let opts = WindowOptions {
        title: "slate IME demo — type something (switch IME to test CJK)".into(),
        size: (800, 400),
        min_size: Some((520, 280)),
        resizable: true,
        ..Default::default()
    };

    App::new(opts).run(|cx: &AppContext| ImeDemo {
        value: Signal::new(cx.runtime(), String::new()),
    });
}
