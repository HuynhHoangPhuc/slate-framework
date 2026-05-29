//! 1k-line TextArea — stress test for text shaping cache + glyph atlas.
//!
//! Renders a `TextArea` seeded with ~1000 lines of mixed-script content.
//! Bench iter calls `mutate()` to bump a trailing line counter, which forces
//! re-shaping of the tail line every frame while the bulk stays cached.

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, Color, Div, FlexDirection, HeadlessApp, IntoAny, JustifyContent,
    TextArea, TextAreaStyle, View,
};

const LINE_COUNT: usize = 1_000;

fn seed_text() -> String {
    let mut buf = String::with_capacity(LINE_COUNT * 48);
    for i in 0..LINE_COUNT {
        // Mix Latin + Arabic so bidi + shaping paths get exercised.
        if i.is_multiple_of(7) {
            buf.push_str(&format!("line {i} مرحبا بالعالم\n"));
        } else {
            buf.push_str(&format!("line {i} the quick brown fox jumps\n"));
        }
    }
    buf
}

pub struct TextAreaView {
    value: Signal<String>,
    base: String,
    tick: u64,
}

impl TextAreaView {
    /// Append a counter to the tail of the buffer so re-shape engages on each
    /// iter while the rest of the cache stays warm.
    pub fn mutate(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        let mut next = self.base.clone();
        next.push_str(&format!("tick {}\n", self.tick));
        self.value.set(next);
    }
}

impl View for TextAreaView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let area_style = TextAreaStyle {
            font_size: 14.0,
            color: Color::WHITE.into(),
            background: Some([0.15, 0.15, 0.18, 1.0]),
            caret_color: Color::WHITE.into(),
            selection_color: [0.4, 0.6, 1.0, 0.3],
            preedit_underline_color: Color::WHITE.into(),
            preedit_selection_color: [0.4, 0.6, 1.0, 0.3],
            width: 800.0,
            min_lines: 30,
        };

        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK))
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .padding_all(16.0)
                    .flex_grow(1.0)
            })
            .child(TextArea::new(self.value.clone()).style(area_style))
            .into_any()
    }
}

pub fn build(app: &HeadlessApp) -> TextAreaView {
    let base = seed_text();
    let value = Signal::new(app.runtime(), base.clone());
    TextAreaView {
        value,
        base,
        tick: 0,
    }
}
