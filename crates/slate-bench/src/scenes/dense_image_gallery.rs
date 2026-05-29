//! Dense image gallery — exercises image-atlas eviction + scroll-back.
//!
//! Mirrors `examples/dense-image-gallery` but headless and scroll-by-set
//! instead of timer-driven, so a bench iter can advance the window
//! deterministically.

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, Color, Div, FlexDirection, HeadlessApp, Image, IntoAny, JustifyContent,
    Text, View,
};

const IMG_SIZE: u32 = 256;
const TOTAL_IMAGES: usize = 120;
const VISIBLE: usize = 3;

fn hue_rgb(h: f32) -> (f32, f32, f32) {
    let h6 = (h * 6.0).rem_euclid(6.0);
    let x = 1.0 - (h6 % 2.0 - 1.0).abs();
    match h6 as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    }
}

fn generate_image(i: usize) -> Vec<u8> {
    let (r, g, b) = hue_rgb(i as f32 / TOTAL_IMAGES as f32);
    let mut bytes = Vec::with_capacity((IMG_SIZE * IMG_SIZE * 4) as usize);
    for y in 0..IMG_SIZE {
        for x in 0..IMG_SIZE {
            let band = ((x + y) / 32 + i as u32).is_multiple_of(2);
            let scale = if band { 255 } else { 150 };
            bytes.extend_from_slice(&[
                (r * scale as f32) as u8,
                (g * scale as f32) as u8,
                (b * scale as f32) as u8,
                255,
            ]);
        }
    }
    bytes
}

pub struct GalleryView {
    images: Vec<Vec<u8>>,
    offset: Signal<usize>,
}

impl GalleryView {
    /// Advance the visible window one slot — bench iter wraps this so the
    /// atlas eviction path engages every frame.
    pub fn advance(&self) {
        self.offset.update(|o| *o = (*o + 1) % TOTAL_IMAGES);
    }
}

impl View for GalleryView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let offset = self.offset.get();
        let mut row = Div::new().style(|s| {
            s.flex_direction(FlexDirection::Row)
                .align_items(AlignItems::Center)
                .gap(12.0)
        });
        for k in 0..VISIBLE {
            let idx = (offset + k) % TOTAL_IMAGES;
            row = row.child(Image::new(IMG_SIZE, IMG_SIZE, self.images[idx].clone()));
        }

        Div::new()
            .background(Color::from_hex("#0d0d14").unwrap_or(Color::BLACK))
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(16.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new(format!("offset {offset}"))
                    .font_size(16.0)
                    .color(Color::WHITE.into()),
            )
            .child(row)
            .into_any()
    }
}

pub fn build(app: &HeadlessApp) -> GalleryView {
    let images = (0..TOTAL_IMAGES).map(generate_image).collect();
    let offset = Signal::new(app.runtime(), 0usize);
    GalleryView { images, offset }
}
