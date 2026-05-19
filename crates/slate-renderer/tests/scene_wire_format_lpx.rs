//! Phase 2 contract test: scene instance structs use `Lpx` for spatial fields.
//!
//! Locks in:
//! - `RectInstance.rect`, `ShadowInstance.rect`, `ImageInstance.rect`,
//!   `GlyphInstance.rect` are `[Lpx; 4]`.
//! - `RectInstance.corner_radius`, `ShadowInstance.corner_radius`,
//!   `ShadowInstance.blur_radius` are `Lpx`.
//! - `ViewportUniform.size` is `[Lpx; 2]`.
//! - Sizes/alignments are byte-for-byte unchanged from the pre-migration
//!   layout, and `Pod` round-trips remain valid (required by every
//!   `bytemuck::cast_slice` upload path).

use slate_renderer::{
    GlyphInstance, ImageInstance, Lpx, RectInstance, ShadowInstance, ViewportUniform,
};

#[test]
fn rect_instance_fields_are_lpx_typed() {
    let r = RectInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(100.0), Lpx(100.0)],
        color: [1.0, 0.0, 0.0, 1.0],
        corner_radius: Lpx(4.0),
        _pad: [0.0; 3],
    };
    assert_eq!(r.rect[2], Lpx(100.0));
    assert_eq!(r.corner_radius, Lpx(4.0));
    assert_eq!(core::mem::size_of::<RectInstance>(), 48);
    assert_eq!(core::mem::align_of::<RectInstance>(), 4);
    let _bytes: &[u8] = bytemuck::bytes_of(&r);
}

#[test]
fn shadow_instance_fields_are_lpx_typed() {
    let s = ShadowInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(50.0), Lpx(50.0)],
        color: [0.0, 0.0, 0.0, 0.5],
        corner_radius: Lpx(6.0),
        blur_radius: Lpx(8.0),
        _pad: [0.0; 2],
    };
    assert_eq!(s.blur_radius, Lpx(8.0));
    assert_eq!(core::mem::size_of::<ShadowInstance>(), 48);
    let _bytes: &[u8] = bytemuck::bytes_of(&s);
}

#[test]
fn image_instance_rect_is_lpx_typed() {
    let i = ImageInstance {
        rect: [Lpx(10.0), Lpx(20.0), Lpx(64.0), Lpx(64.0)],
        uv_rect: [0.0, 0.0, 1.0, 1.0],
        tint: [1.0; 4],
    };
    assert_eq!(i.rect[0], Lpx(10.0));
    assert_eq!(core::mem::size_of::<ImageInstance>(), 48);
    let _bytes: &[u8] = bytemuck::bytes_of(&i);
}

#[test]
fn glyph_instance_rect_is_lpx_typed() {
    let g = GlyphInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(8.0), Lpx(16.0)],
        uv_rect: [0.0, 0.0, 0.5, 0.5],
        color: [1.0; 4],
        sub_pixel_variant: 0,
        _pad: [0; 3],
    };
    assert_eq!(g.rect[3], Lpx(16.0));
    assert_eq!(core::mem::size_of::<GlyphInstance>(), 64);
    let _bytes: &[u8] = bytemuck::bytes_of(&g);
}

#[test]
fn viewport_uniform_size_is_lpx_typed() {
    let v = ViewportUniform {
        size: [Lpx(800.0), Lpx(600.0)],
        _pad: [0.0; 2],
    };
    assert_eq!(v.size[0], Lpx(800.0));
    assert_eq!(core::mem::size_of::<ViewportUniform>(), 16);
    let _bytes: &[u8] = bytemuck::bytes_of(&v);
}

#[test]
fn rect_instance_round_trips_through_bytemuck_cast_slice() {
    let xs = [RectInstance {
        rect: [Lpx(1.0), Lpx(2.0), Lpx(3.0), Lpx(4.0)],
        color: [0.0; 4],
        corner_radius: Lpx(0.0),
        _pad: [0.0; 3],
    }; 4];
    let raw: &[u8] = bytemuck::cast_slice(&xs);
    assert_eq!(raw.len(), 4 * core::mem::size_of::<RectInstance>());
}
