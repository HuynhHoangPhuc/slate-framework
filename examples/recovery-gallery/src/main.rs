//! recovery-gallery — Image + device-lost recovery showcase.
//!
//! Demonstrates the framework's Image element with automatic device-lost
//! recovery via App::new + AppState. Drag the window across monitors on a
//! hybrid-GPU system to exercise the recovery state machine.
//!
//! Run: `cargo run -p recovery-gallery`

use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, Image, IntoAny,
    JustifyContent, Text, View, WindowOptions,
};

const IMG_SIZE: u32 = 256;

/// Generate a 256x256 checkerboard pattern (sRGB u8, straight alpha).
fn generate_checkerboard() -> Vec<u8> {
    let mut bytes = Vec::with_capacity((IMG_SIZE * IMG_SIZE * 4) as usize);
    for y in 0..IMG_SIZE {
        for x in 0..IMG_SIZE {
            let cell = ((x / 32) + (y / 32)) % 2 == 0;
            let v: u8 = if cell { 240 } else { 64 };
            bytes.extend_from_slice(&[v, v, v, 255]);
        }
    }
    bytes
}

struct GalleryView {
    checkerboard: Vec<u8>,
}

impl GalleryView {
    fn new() -> Self {
        Self {
            checkerboard: generate_checkerboard(),
        }
    }
}

impl View for GalleryView {
    fn render(&mut self) -> AnyElement {
        Div::new()
            .background(Color::from_hex("#0d0d14").unwrap_or(Color::BLACK).into())
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(16.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new("recovery-gallery")
                    .font_size(20.0)
                    .color(Color::WHITE.into()),
            )
            .child(Image::new(IMG_SIZE, IMG_SIZE, self.checkerboard.clone()))
            .child(
                Text::new("Drag this window across monitors to test recovery")
                    .font_size(12.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · recovery-gallery".into(),
        size: (800, 600),
        min_size: Some((320, 240)),
        resizable: true,
        ..Default::default()
    })
    .run(|_cx: &AppContext| GalleryView::new());
}
