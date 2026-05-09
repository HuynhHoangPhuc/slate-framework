//! Anchor Spike — Markdown to PNG headless renderer.
//!
//! Phase 3.5 validation: exercises Element API ergonomics with a non-trivial
//! example before Phase 4 (signals) commits the framework's shape.
//!
//! Usage: `cargo run -p anchor-md-to-png -- input.md output.png`

use std::env;
use std::fs;

use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use slate_framework::{
    AnyElement, Color, Div, Edges, FlexDirection, HeadlessApp, IntoAny, Length, Text, TextWrap,
};

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: {} <input.md> <output.png>", args[0]);
        std::process::exit(1);
    }

    let input_path = &args[1];
    let output_path = &args[2];

    if let Err(e) = run(input_path, output_path) {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }
}

fn run(input_path: &str, output_path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let markdown = fs::read_to_string(input_path)?;

    log::info!("Parsing markdown from: {input_path}");
    let element = md_to_element(&markdown);

    log::info!("Creating headless renderer (800x600)");
    let mut app = HeadlessApp::new(800, 600)?;

    log::info!("Rendering element tree");
    let image = app.render(element)?;

    log::info!("Saving PNG to: {output_path}");
    image.save(output_path)?;

    log::info!("Done! Output: {output_path}");
    Ok(())
}

/// Convert markdown text to an element tree.
fn md_to_element(text: &str) -> AnyElement {
    let parser = Parser::new(text);
    let mut builder = ElementBuilder::new();

    for event in parser {
        builder.handle_event(event);
    }

    builder.finish()
}

/// Build a Div container from children.
fn build_div_with_children(
    style_fn: impl FnOnce(slate_framework::Style) -> slate_framework::Style,
    background: Option<[f32; 4]>,
    corner_radius: f32,
    children: Vec<AnyElement>,
) -> Div {
    let mut div = Div::new().style(style_fn);

    if let Some(bg) = background {
        div = div.background(bg);
    }

    if corner_radius > 0.0 {
        div = div.corner_radius(corner_radius);
    }

    for child in children {
        div = div.child_any(child);
    }

    div
}

/// Markdown → Element tree builder.
struct ElementBuilder {
    stack: Vec<ContainerState>,
    text_buf: String,
}

struct ContainerState {
    kind: ContainerKind,
    children: Vec<AnyElement>,
}

#[derive(Clone, Copy)]
enum ContainerKind {
    Root,
    Heading(HeadingLevel),
    Paragraph,
    BlockQuote,
    CodeBlock,
    List,
    ListItem,
}

impl ElementBuilder {
    fn new() -> Self {
        Self {
            stack: vec![ContainerState {
                kind: ContainerKind::Root,
                children: Vec::new(),
            }],
            text_buf: String::new(),
        }
    }

    fn handle_event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(text) => self.text_buf.push_str(&text),
            Event::Code(code) => {
                self.text_buf.push('`');
                self.text_buf.push_str(&code);
                self.text_buf.push('`');
            }
            Event::SoftBreak | Event::HardBreak => self.text_buf.push(' '),
            _ => {}
        }
    }

    fn start_tag(&mut self, tag: Tag) {
        match tag {
            Tag::Heading { level, .. } => {
                self.flush_text();
                self.push_container(ContainerKind::Heading(level));
            }
            Tag::Paragraph => {
                self.flush_text();
                self.push_container(ContainerKind::Paragraph);
            }
            Tag::BlockQuote(_) => {
                self.flush_text();
                self.push_container(ContainerKind::BlockQuote);
            }
            Tag::CodeBlock(_) => {
                self.flush_text();
                self.push_container(ContainerKind::CodeBlock);
            }
            Tag::List(_) => {
                self.flush_text();
                self.push_container(ContainerKind::List);
            }
            Tag::Item => {
                self.flush_text();
                self.push_container(ContainerKind::ListItem);
            }
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Heading(_) | TagEnd::Paragraph | TagEnd::BlockQuote(_) | TagEnd::CodeBlock => {
                self.flush_text();
                self.pop_container();
            }
            TagEnd::List(_) => {
                self.flush_text();
                self.pop_container();
            }
            TagEnd::Item => {
                self.flush_text();
                self.pop_container();
            }
            _ => {}
        }
    }

    fn push_container(&mut self, kind: ContainerKind) {
        self.stack.push(ContainerState {
            kind,
            children: Vec::new(),
        });
    }

    fn pop_container(&mut self) {
        let state = self.stack.pop().expect("stack underflow");
        let element = self.build_container(state);
        self.stack
            .last_mut()
            .expect("root missing")
            .children
            .push(element);
    }

    fn flush_text(&mut self) {
        if self.text_buf.trim().is_empty() {
            self.text_buf.clear();
            return;
        }

        let text = std::mem::take(&mut self.text_buf);
        let container = self.stack.last().map(|s| s.kind);

        let element = match container {
            Some(ContainerKind::Heading(level)) => {
                let (font_size, color) = heading_style(level);
                Text::new(text.trim())
                    .font_size(font_size)
                    .color(color)
                    .wrap(TextWrap::Wrap)
                    .into_any()
            }
            Some(ContainerKind::CodeBlock) => Text::new(text.trim())
                .font_size(13.0)
                .color(hex_color("#e0e0e0"))
                .into_any(),
            Some(ContainerKind::BlockQuote) => Text::new(text.trim())
                .font_size(14.0)
                .color(hex_color("#a0a0a0"))
                .wrap(TextWrap::Wrap)
                .into_any(),
            _ => Text::new(text.trim())
                .font_size(14.0)
                .color(Color::WHITE.as_array())
                .wrap(TextWrap::Wrap)
                .into_any(),
        };

        self.stack
            .last_mut()
            .expect("stack empty")
            .children
            .push(element);
    }

    fn build_container(&self, state: ContainerState) -> AnyElement {
        let ContainerState { kind, children } = state;

        match kind {
            ContainerKind::Root => build_div_with_children(
                |s| {
                    s.flex_direction(FlexDirection::Column)
                        .padding_all(40.0)
                        .gap(16.0)
                },
                Some(hex_color("#1a1a2e")),
                0.0,
                children,
            )
            .into_any(),

            ContainerKind::Heading(level) => {
                let gap = match level {
                    HeadingLevel::H1 => 12.0,
                    HeadingLevel::H2 => 8.0,
                    _ => 4.0,
                };
                build_div_with_children(
                    |s| s.flex_direction(FlexDirection::Column).gap(gap),
                    None,
                    0.0,
                    children,
                )
                .into_any()
            }

            ContainerKind::Paragraph => build_div_with_children(
                |s| {
                    s.flex_direction(FlexDirection::Column)
                        .padding(Edges::symmetric(Length::Px(4.0), Length::Px(0.0)))
                },
                None,
                0.0,
                children,
            )
            .into_any(),

            ContainerKind::BlockQuote => build_div_with_children(
                |s| {
                    s.flex_direction(FlexDirection::Column).padding(Edges {
                        left: Length::Px(16.0),
                        top: Length::Px(8.0),
                        bottom: Length::Px(8.0),
                        right: Length::Px(8.0),
                    })
                },
                Some(hex_color("#252540")),
                4.0,
                children,
            )
            .into_any(),

            ContainerKind::CodeBlock => build_div_with_children(
                |s| s.flex_direction(FlexDirection::Column).padding_all(12.0),
                Some(hex_color("#0d0d1a")),
                6.0,
                children,
            )
            .into_any(),

            ContainerKind::List => build_div_with_children(
                |s| {
                    s.flex_direction(FlexDirection::Column)
                        .padding(Edges {
                            left: Length::Px(20.0),
                            ..Default::default()
                        })
                        .gap(4.0)
                },
                None,
                0.0,
                children,
            )
            .into_any(),

            ContainerKind::ListItem => {
                let bullet = Text::new("•").font_size(14.0).color(hex_color("#666688"));

                let content = build_div_with_children(
                    |s| s.flex_direction(FlexDirection::Column).flex_grow(1.0),
                    None,
                    0.0,
                    children,
                );

                Div::new()
                    .style(|s| s.flex_direction(FlexDirection::Row).gap(8.0))
                    .child(bullet)
                    .child(content)
                    .into_any()
            }
        }
    }

    fn finish(mut self) -> AnyElement {
        self.flush_text();

        while self.stack.len() > 1 {
            self.pop_container();
        }

        let root = self.stack.pop().expect("root missing");
        self.build_container(root)
    }
}

fn heading_style(level: HeadingLevel) -> (f32, [f32; 4]) {
    match level {
        HeadingLevel::H1 => (32.0, hex_color("#ffffff")),
        HeadingLevel::H2 => (24.0, hex_color("#e8e8ff")),
        HeadingLevel::H3 => (18.0, hex_color("#c8c8e8")),
        _ => (16.0, hex_color("#b0b0d0")),
    }
}

fn hex_color(hex: &str) -> [f32; 4] {
    Color::from_hex(hex).unwrap_or(Color::WHITE).as_array()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_markdown() {
        let md = "# Hello\n\nThis is a paragraph.";
        let _element = md_to_element(md);
    }

    #[test]
    fn parse_complex_markdown() {
        let md = r#"
# Heading 1

This is a paragraph with **bold** and *italic* text.

## Heading 2

- Item 1
- Item 2
- Item 3

> This is a blockquote

```
code block
```
"#;
        let _element = md_to_element(md);
    }
}
