use slate_renderer::{
    GlyphInstance, ImageInstance, RectInstance, Scene, ShadowInstance, srgb_u8_to_linear_premul,
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
/// `text_glyphs` contains pre-built GlyphInstances from the native text pipeline.
pub fn build_scene(
    scene: &mut Scene,
    w: f32,
    h: f32,
    scale: f32,
    image_uv: [f32; 4],
    text_glyphs: &[GlyphInstance],
) {
    scene.clear();

    // Layer 0: background + shadows + rects
    scene.push_layer();
    scene.push_rect(RectInstance {
        rect: [0.0, 0.0, w, h],
        color: srgb_u8_to_linear_premul([0x1A, 0x1A, 0x26, 0xFF]),
        corner_radius: 0.0,
        _pad: [0.0; 3],
    });

    // Adaptive color grid: columns based on available width
    let w_lpx = w / scale;
    let inset = 30.0;
    let cell = 48.0;
    let gap = 12.0;
    let stride_lpx = cell + gap;
    let cols = ((w_lpx - 2.0 * inset) / stride_lpx).floor().max(1.0) as usize;

    let size = cell * scale;
    let stride = stride_lpx * scale;
    let ox = inset * scale;
    let oy = 40.0 * scale;

    // Dark rectangle for light-on-dark text demo (right-anchored)
    let dark_w = 200.0;
    let dark_x = (w_lpx - dark_w - 20.0).max(20.0);
    scene.push_rect(RectInstance {
        rect: [dark_x * scale, 175.0 * scale, dark_w * scale, 40.0 * scale],
        color: srgb_u8_to_linear_premul([0x1E, 0x1E, 0x2E, 0xFF]),
        corner_radius: 6.0 * scale,
        _pad: [0.0; 3],
    });

    // Cap shadow count to fit adaptive grid (30 shadows, but limit to visible rows)
    let shadow_count = 30.min(cols * 5);
    for i in 0..shadow_count {
        scene.push_shadow(ShadowInstance {
            rect: [
                ox + (i % cols) as f32 * stride + 4.0 * scale,
                oy + (i / cols) as f32 * stride + 4.0 * scale,
                size,
                size,
            ],
            color: srgb_u8_to_linear_premul([0, 0, 0, 0x80]),
            corner_radius: ((i % 5) as f32 + 1.0) * 2.0 * scale,
            blur_radius: 8.0 * scale,
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
                ox + (i % cols) as f32 * stride,
                oy + (i / cols) as f32 * stride,
                size,
                size,
            ],
            color: srgb_u8_to_linear_premul([r, g, b, 0xCC]),
            corner_radius: ((i % 5) as f32 + 1.0) * 2.0 * scale,
            _pad: [0.0; 3],
        });
    }

    // Layer 1: images + text glyphs
    scene.push_layer();
    push_images(scene, image_uv, scale, w, h);

    for g in text_glyphs {
        scene.push_glyph(*g);
    }
}

fn push_images(scene: &mut Scene, image_uv: [f32; 4], scale: f32, w: f32, h: f32) {
    // Adaptive image grid: columns based on available width
    let w_lpx = w / scale;
    let inset = 40.0;
    let img_cell = 64.0;
    let img_gap = 20.0;
    let img_stride_lpx = img_cell + img_gap;
    let cols = ((w_lpx - 2.0 * inset) / img_stride_lpx).floor().max(1.0) as usize;

    let size = img_cell * scale;
    let stride = img_stride_lpx * scale;
    let ox = inset * scale;
    let oy = h * 0.55;

    for i in 0..20 {
        let hue = i as f32 / 20.0 * 360.0;
        let (r, g, b) = hsv_to_srgb(hue, 0.3, 1.0);
        scene.push_image(ImageInstance {
            rect: [
                ox + (i % cols) as f32 * stride,
                oy + (i / cols) as f32 * stride,
                size,
                size,
            ],
            uv_rect: image_uv,
            tint: srgb_u8_to_linear_premul([r, g, b, 0xFF]),
        });
    }
}
