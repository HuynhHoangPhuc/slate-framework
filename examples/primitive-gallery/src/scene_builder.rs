use slate_renderer::{
    GlyphInstance, ImageInstance, Lpx, RectInstance, Scene, ShadowInstance,
    srgb_u8_to_linear_premul,
};

pub const IMG_SIZE: u32 = 256;

pub fn generate_checkerboard() -> Vec<u8> {
    let tile = 32u32;
    let mut px = Vec::with_capacity((IMG_SIZE * IMG_SIZE * 4) as usize);
    for y in 0..IMG_SIZE {
        for x in 0..IMG_SIZE {
            let (r, g, b) = if ((x / tile) + (y / tile)).is_multiple_of(2) {
                (0x66u8, 0xCC, 0xFF)
            } else {
                (0xFF, 0x99, 0x33)
            };
            px.extend_from_slice(&[r, g, b, 0xFF]);
        }
    }
    px
}

fn hsv_to_srgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

/// Build scene with rects, shadows, images, and text glyphs.
///
/// All coordinates are in logical pixels (lpx); the renderer's viewport
/// uniform maps lpx → NDC. `text_glyphs` already carry lpx values.
pub fn build_scene(
    scene: &mut Scene,
    w_lpx: f32,
    h_lpx: f32,
    image_uv: [f32; 4],
    text_glyphs: &[GlyphInstance],
) {
    scene.clear();

    // Layer 0: background + shadows + rects
    scene.push_layer();
    scene.push_rect(RectInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w_lpx), Lpx(h_lpx)],
        color: srgb_u8_to_linear_premul([0x1A, 0x1A, 0x26, 0xFF]),
        corner_radius: Lpx(0.0),
        _pad: [0.0; 3],
    });

    // Adaptive color grid: columns based on available width
    let inset = 30.0f32;
    let cell = 48.0f32;
    let gap = 12.0f32;
    let stride = cell + gap;
    let cols = ((w_lpx - 2.0 * inset) / stride).floor().max(1.0) as usize;

    let size = cell;
    let ox = inset;
    let oy = 40.0f32;

    // Dark rectangle for light-on-dark text demo (right-anchored)
    let dark_w = 200.0f32;
    let dark_x = (w_lpx - dark_w - 20.0).max(20.0);
    scene.push_rect(RectInstance {
        rect: [Lpx(dark_x), Lpx(175.0), Lpx(dark_w), Lpx(40.0)],
        color: srgb_u8_to_linear_premul([0x1E, 0x1E, 0x2E, 0xFF]),
        corner_radius: Lpx(6.0),
        _pad: [0.0; 3],
    });

    // Cap shadow count to fit adaptive grid (30 shadows, but limit to visible rows)
    let shadow_count = 30.min(cols * 5);
    for i in 0..shadow_count {
        scene.push_shadow(ShadowInstance {
            rect: [
                Lpx(ox + (i % cols) as f32 * stride + 4.0),
                Lpx(oy + (i / cols) as f32 * stride + 4.0),
                Lpx(size),
                Lpx(size),
            ],
            color: srgb_u8_to_linear_premul([0, 0, 0, 0x80]),
            corner_radius: Lpx(((i % 5) as f32 + 1.0) * 2.0),
            blur_radius: Lpx(8.0),
            _pad: [0.0; 2],
        });
    }

    // Cap rect count to available grid space
    let rect_count = 50.min(cols * 6);
    for i in 0..rect_count {
        let hue = i as f32 / 50.0 * 360.0;
        let (r, g, b) = hsv_to_srgb(hue, 0.7, 0.9);
        scene.push_rect(RectInstance {
            rect: [
                Lpx(ox + (i % cols) as f32 * stride),
                Lpx(oy + (i / cols) as f32 * stride),
                Lpx(size),
                Lpx(size),
            ],
            color: srgb_u8_to_linear_premul([r, g, b, 0xCC]),
            corner_radius: Lpx(((i % 5) as f32 + 1.0) * 2.0),
            _pad: [0.0; 3],
        });
    }

    // Layer 1: images + text glyphs
    scene.push_layer();
    push_images(scene, image_uv, w_lpx, h_lpx);

    for g in text_glyphs {
        scene.push_glyph(*g);
    }
}

fn push_images(scene: &mut Scene, image_uv: [f32; 4], w_lpx: f32, h_lpx: f32) {
    // Adaptive image grid: columns based on available width
    let inset = 40.0f32;
    let img_cell = 64.0f32;
    let img_gap = 20.0f32;
    let stride = img_cell + img_gap;
    let cols = ((w_lpx - 2.0 * inset) / stride).floor().max(1.0) as usize;

    let size = img_cell;
    let ox = inset;
    let oy = h_lpx * 0.55;

    for i in 0..20 {
        let hue = i as f32 / 20.0 * 360.0;
        let (r, g, b) = hsv_to_srgb(hue, 0.3, 1.0);
        scene.push_image(ImageInstance {
            rect: [
                Lpx(ox + (i % cols) as f32 * stride),
                Lpx(oy + (i / cols) as f32 * stride),
                Lpx(size),
                Lpx(size),
            ],
            uv_rect: image_uv,
            tint: srgb_u8_to_linear_premul([r, g, b, 0xFF]),
        });
    }
}
