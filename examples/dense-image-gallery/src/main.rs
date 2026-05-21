//! dense-image-gallery — atlas eviction + scroll-back showcase.
//!
//! Generates more distinct images than a single atlas page can hold, then
//! scrolls a small visible window across all of them on a timer. Images
//! scrolled off-screen go untouched for a frame and are evicted; when the
//! window wraps back around, those same images are re-requested. This is the
//! real consumer of the eviction-safety fix: the image cache gates each
//! cache-hit on a monotonic liveness token, so a re-requested image whose
//! atlas slot was evicted (and possibly reused by another image) is detected
//! as stale and re-uploaded — never sampled from the wrong texture.
//!
//! Run: `cargo run -p dense-image-gallery`

use std::time::Duration;

use slate_framework::reactive::Signal;
use slate_framework::{
    AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, Image, IntoAny,
    JustifyContent, Text, Timer, View, WindowOptions,
};

const IMG_SIZE: u32 = 256;
/// Far more than the ~49 padded 256² tiles a 2048² page holds, so scrolling
/// forces real atlas eviction.
const TOTAL_IMAGES: usize = 120;
/// Images shown side-by-side at any moment.
const VISIBLE: usize = 3;
/// Scroll one slot every tick.
const SCROLL_INTERVAL: Duration = Duration::from_millis(250);

/// A distinct 256×256 image for index `i` (sRGB u8, straight alpha). Hue is
/// derived from the index so every image's pixels — and thus its content hash —
/// are unique, giving each its own atlas slot.
fn generate_image(i: usize) -> Vec<u8> {
    let (r, g, b) = hue_rgb(i as f32 / TOTAL_IMAGES as f32);
    let mut bytes = Vec::with_capacity((IMG_SIZE * IMG_SIZE * 4) as usize);
    for y in 0..IMG_SIZE {
        for x in 0..IMG_SIZE {
            // A diagonal band keeps the pattern non-uniform so a wrong-texture
            // sample would be visually obvious.
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

/// Minimal HSV→RGB at full saturation/value; returns channels in `[0, 1]`.
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

struct GalleryView {
    images: Vec<Vec<u8>>,
    offset: Signal<usize>,
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
                Text::new(format!(
                    "dense-image-gallery · showing {}..{} of {}",
                    offset,
                    offset + VISIBLE,
                    TOTAL_IMAGES
                ))
                .font_size(16.0)
                .color(Color::WHITE.into()),
            )
            .child(row)
            .child(
                Text::new("scrolls automatically — exercises atlas eviction + scroll-back")
                    .font_size(12.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · dense-image-gallery".into(),
        size: (900, 480),
        min_size: Some((320, 240)),
        resizable: true,
        ..Default::default()
    })
    .run(|cx: &AppContext| {
        let images = (0..TOTAL_IMAGES).map(generate_image).collect();
        let offset = Signal::new(cx.runtime(), 0usize);

        // Advance the visible window on a timer so off-screen images get
        // evicted and scrolled-back images are re-requested.
        let scroll = offset.clone();
        cx.background_executor()
            .spawn(async move {
                loop {
                    Timer::after(SCROLL_INTERVAL).await;
                    scroll.update(|o| *o = (*o + 1) % TOTAL_IMAGES);
                }
            })
            .detach();

        GalleryView { images, offset }
    });
}
