//! textarea — multi-line `TextArea` demo: wrapping, newline editing, vertical
//! caret nav, selection, word/line click-select, clipboard, undo/redo, and IME.
//!
//! - Click the area to focus, then type. Text wraps at the fixed content width;
//!   Enter inserts a hard newline.
//! - ↑/↓ move between visual lines (the column is sticky); Home/End jump to the
//!   visual line's edges.
//! - Click places the caret; drag selects across lines; double-click selects a
//!   word; triple-click selects the visual line.
//! - Cmd/Ctrl + C/X/V copy / cut / paste (newlines survive a multi-line paste).
//! - Cmd/Ctrl + Z undo, Shift+Z (macOS) / Y (Windows/Linux) redo.
//! - Switch to a CJK IME and compose mid-document; the caret blinks while
//!   focused. "You typed:" reactively echoes the bound `Signal<String>`.
//!
//! Run: `cargo run -p textarea`

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny, JustifyContent,
    Text, TextArea, TextAreaStyle, View, WindowOptions,
};

#[cfg(target_os = "macos")]
const SHORTCUTS_HINT: &str = "Shortcuts: \u{2318}C copy \u{00b7} \u{2318}X cut \u{00b7} \
                              \u{2318}V paste \u{00b7} \u{2318}Z undo \u{00b7} \
                              \u{21e7}\u{2318}Z redo \u{00b7} \u{21e7}+arrows select \u{00b7} \
                              double=word \u{00b7} triple=line \u{00b7} drag to select";
#[cfg(not(target_os = "macos"))]
const SHORTCUTS_HINT: &str = "Shortcuts: Ctrl+C copy \u{00b7} Ctrl+X cut \u{00b7} \
                              Ctrl+V paste \u{00b7} Ctrl+Z undo \u{00b7} Ctrl+Y redo \u{00b7} \
                              Shift+arrows select \u{00b7} double=word \u{00b7} triple=line \u{00b7} \
                              drag to select";

struct TextAreaDemo {
    value: Signal<String>,
}

impl View for TextAreaDemo {
    fn render(&mut self) -> AnyElement {
        let echo = self.value.clone();

        let area_style = TextAreaStyle {
            font_size: 18.0,
            color: Color::WHITE.into(),
            background: Some([0.15, 0.15, 0.18, 1.0]),
            caret_color: Color::WHITE.into(),
            selection_color: [0.4, 0.6, 1.0, 0.3],
            preedit_underline_color: Color::WHITE.into(),
            preedit_selection_color: [0.4, 0.6, 1.0, 0.3],
            width: 520.0,
            min_lines: 6,
        };

        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK).into())
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(18.0)
                    .padding_all(32.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new("Slate TextArea — click the area, then type (Enter = new line)")
                    .font_size(16.0)
                    .color(Color::WHITE.into()),
            )
            .child(TextArea::new(self.value.clone()).style(area_style))
            .child(
                Text::new_reactive(move || {
                    let v = echo.get();
                    if v.is_empty() {
                        "You typed: (empty)".to_string()
                    } else {
                        // Show line count so wrapping/newlines are visible at a glance.
                        let lines = v.split('\n').count();
                        format!("You typed {} char(s) across {lines} line(s)", v.len())
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
                    "Switch IME — macOS: Ctrl+Space \u{00b7} Windows: Win+Space. \
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
        title: "slate TextArea — wrap, newline, nav, select, clipboard, IME".into(),
        size: (900, 560),
        min_size: Some((620, 360)),
        resizable: true,
        ..Default::default()
    };

    App::new(opts).run(|cx: &AppContext| TextAreaDemo {
        value: Signal::new(cx.runtime(), String::new()),
    });
}
