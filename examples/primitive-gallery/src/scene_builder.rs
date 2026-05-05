use slate_renderer::{
    GlyphInstance, ImageInstance, RectInstance, Scene, ShadowInstance, srgb_u8_to_linear_premul,
};

pub const GLYPH_W: u32 = 5;
pub const GLYPH_H: u32 = 7;
pub const IMG_SIZE: u32 = 256;

#[rustfmt::skip]
pub const DIGIT_BITS: [u64; 11] = [
    0b01110_10001_10001_10001_10001_10001_01110, // 0
    0b00100_01100_00100_00100_00100_00100_01110, // 1
    0b01110_10001_00001_00010_00100_01000_11111, // 2
    0b01110_10001_00001_00110_00001_10001_01110, // 3
    0b00010_00110_01010_10010_11111_00010_00010, // 4
    0b11111_10000_11110_00001_00001_10001_01110, // 5
    0b01110_10000_10000_11110_10001_10001_01110, // 6
    0b11111_00001_00010_00100_01000_01000_01000, // 7
    0b01110_10001_10001_01110_10001_10001_01110, // 8
    0b01110_10001_10001_01111_00001_00001_01110, // 9
    0b00000_00000_00000_00000_00000_00100_00100, // .
];

pub fn padded_glyph_pixels(bits: u64) -> Vec<u8> {
    let pw = (GLYPH_W + 2) as usize;
    let ph = (GLYPH_H + 2) as usize;
    let mut buf = vec![0u8; pw * ph];
    for row in 0..GLYPH_H as usize {
        for col in 0..GLYPH_W as usize {
            let bit_idx = row * GLYPH_W as usize + col;
            if (bits >> (34 - bit_idx)) & 1 == 1 {
                buf[(row + 1) * pw + (col + 1)] = 0xFF;
            }
        }
    }
    buf
}

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

pub fn build_scene(
    scene: &mut Scene,
    w: f32,
    h: f32,
    scale: f32,
    image_uv: [f32; 4],
    digit_uvs: &[[f32; 4]],
    fps: f32,
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

    let cols = 10usize;
    let size = 48.0 * scale;
    let stride = size + 12.0 * scale;
    let ox = 30.0 * scale;
    let oy = 40.0 * scale;

    for i in 0..30 {
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

    for i in 0..50 {
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

    // Layer 1: images + glyphs
    scene.push_layer();
    push_images(scene, image_uv, scale, h);
    push_decorative_glyphs(scene, digit_uvs, scale, w);
    push_fps_overlay(scene, digit_uvs, fps, scale, w);
}

fn push_images(scene: &mut Scene, image_uv: [f32; 4], scale: f32, h: f32) {
    let cols = 5usize;
    let size = 64.0 * scale;
    let stride = size + 20.0 * scale;
    let ox = 40.0 * scale;
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

fn push_decorative_glyphs(scene: &mut Scene, digit_uvs: &[[f32; 4]], scale: f32, w: f32) {
    let gw = GLYPH_W as f32 * scale * 3.0;
    let gh = GLYPH_H as f32 * scale * 3.0;
    let cols = 10usize;
    let gx_stride = gw + 12.0 * scale;
    let gy_stride = gh + 8.0 * scale;
    let ox = w * 0.55;
    let oy = 50.0 * scale;
    for i in 0..50 {
        let hue = i as f32 / 50.0 * 360.0;
        let (r, g, b) = hsv_to_srgb(hue, 0.5, 1.0);
        scene.push_glyph(GlyphInstance {
            rect: [ox + (i % cols) as f32 * gx_stride, oy + (i / cols) as f32 * gy_stride, gw, gh],
            uv_rect: digit_uvs[i % 10],
            color: srgb_u8_to_linear_premul([r, g, b, 0xFF]),
            sub_pixel_variant: 0,
            _pad: [0; 3],
        });
    }
}

fn push_fps_overlay(scene: &mut Scene, digit_uvs: &[[f32; 4]], fps: f32, scale: f32, w: f32) {
    let fps_text = format!("{fps:.1}");
    let gw = GLYPH_W as f32 * scale * 2.5;
    let gh = GLYPH_H as f32 * scale * 2.5;
    let spacing = gw + 2.0 * scale;
    let x = w - fps_text.len() as f32 * spacing - 10.0 * scale;
    let y = 10.0 * scale;
    for (j, ch) in fps_text.chars().enumerate() {
        let idx = match ch {
            '0'..='9' => (ch as u8 - b'0') as usize,
            '.' => 10,
            _ => continue,
        };
        scene.push_glyph(GlyphInstance {
            rect: [x + j as f32 * spacing, y, gw, gh],
            uv_rect: digit_uvs[idx],
            color: srgb_u8_to_linear_premul([0xFF, 0xFF, 0x00, 0xFF]),
            sub_pixel_variant: 0,
            _pad: [0; 3],
        });
    }
}
