//! ime-textfield — TextField v2 demo: IME composition, selection, clipboard,
//! undo/redo, and a blinking caret.
//!
//! - Click the TextField to focus, then type ASCII or switch to a CJK IME.
//! - Shift+Arrow / Shift+Home / Shift+End extend the selection.
//! - Cmd/Ctrl + C/X/V copy / cut / paste through the OS clipboard.
//! - Cmd/Ctrl + Z (Shift+Z on macOS, Y on Windows/Linux) walks the Apple
//!   Notes-style undo history.
//! - Drag-select with the mouse; the caret blinks at 530 ms while focused.
//! - "You typed:" reactively echoes the bound `Signal<String>`.
//! - A mixed LTR/RTL (English + Arabic) sample is preloaded: press ←/→ to walk
//!   the caret across direction boundaries — it always moves the way you press,
//!   not by logical byte order — and Home/End jump to the visual line edges.
//!
//! Run: `cargo run -p ime-textfield`

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, JustifyContent,
    Text, TextField, TextFieldStyle, View, WindowOptions,
};

#[cfg(target_os = "macos")]
const SHORTCUTS_HINT: &str = "Shortcuts: \u{2318}C copy \u{00b7} \u{2318}X cut \u{00b7} \
                              \u{2318}V paste \u{00b7} \u{2318}Z undo \u{00b7} \
                              \u{21e7}\u{2318}Z redo \u{00b7} \u{21e7}+arrows select \u{00b7} \
                              drag to select";
#[cfg(not(target_os = "macos"))]
const SHORTCUTS_HINT: &str = "Shortcuts: Ctrl+C copy \u{00b7} Ctrl+X cut \u{00b7} \
                              Ctrl+V paste \u{00b7} Ctrl+Z undo \u{00b7} Ctrl+Y redo \u{00b7} \
                              Shift+arrows select \u{00b7} drag to select";

/// Preloaded mixed LTR/RTL content (English + Arabic) so visual caret motion,
/// Home/End-to-visual-edge, and per-run selection are visible on launch.
const RTL_SAMPLE: &str = "Hello مرحبا world";

struct ImeDemo {
    value: Signal<String>,
}

impl View for ImeDemo {
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
                    .gap(18.0)
                    .padding_all(32.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new("Slate TextField v2 — click the field, then type")
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
                Text::new(SHORTCUTS_HINT)
                    .font_size(12.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .child(
                Text::new(
                    "BiDi: a mixed English + Arabic sample is preloaded · \
                     ←/→ walk the caret across direction boundaries · \
                     Home/End go to the visual line edges",
                )
                .font_size(12.0)
                .color(Color::from_hex("#64748b").unwrap_or(Color::WHITE).into()),
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
        title: "slate TextField v2 — selection, clipboard, undo, blink".into(),
        size: (840, 440),
        min_size: Some((560, 300)),
        resizable: true,
        ..Default::default()
    };

    App::new(opts).run(|cx: &AppContext| ImeDemo {
        value: Signal::new(cx.runtime(), RTL_SAMPLE.to_string()),
    });
}
