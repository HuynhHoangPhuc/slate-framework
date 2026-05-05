//! End-to-end smoke test for the Phase 1 premultiplied-alpha contract.
//!
//! Renders the rect pipeline (`shaders/rect.wgsl`) into a 32×32 off-screen
//! `Bgra8UnormSrgb` texture with `srgb_u8_to_linear_premul([255, 0, 0, 128])`
//! and asserts the center pixel reads back as roughly `(187, 0, 0, 128)`.
//!
//! Math (matches `phase-01-scene-primitive-types.md` step 5):
//!   linear_premul = (0.502, 0, 0, 0.502)
//!   blend over transparent dst (One/OneMinusSrcAlpha) → (0.502, 0, 0, 0.502)
//!   sRGB encode R: linear_to_srgb(0.502) ≈ 0.737 → ≈188 byte
//!   alpha is linear-passthrough on Unorm → 128 byte
//!
//! Tolerance ±2/255 swallows rounding differences between drivers.

mod common;

use slate_renderer::{RectPipeline, RectUniform, srgb_u8_to_linear_premul};
use wgpu::{
    Color, LoadOp, Operations, RenderPassColorAttachment, RenderPassDescriptor, StoreOp,
    TextureFormat,
};

#[test]
fn premul_red_half_alpha_round_trips_through_pipeline() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("smoke_premul: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let pipeline = RectPipeline::new(&device, format);

    let (w, h) = (32u32, 32u32);
    pipeline.upload(
        &queue,
        RectUniform {
            center: [w as f32 / 2.0, h as f32 / 2.0],
            half_size: [8.0, 8.0],
            color: srgb_u8_to_linear_premul([255, 0, 0, 128]),
            viewport_size: [w as f32, h as f32],
            corner_radius: 0.0,
            _pad: 0.0,
        },
    );

    let buf = common::render_to_texture(&device, &queue, format, w, h, |encoder, view| {
        let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("smoke-pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    // Transparent black — premul invariant: rgb ≤ a, both 0.
                    load: LoadOp::Clear(Color::TRANSPARENT),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        pipeline.record(&mut rpass);
    });

    // Center pixel — well inside the half_size=8 rect, coverage = 1.0.
    common::assert_pixel(&buf, w, w / 2, h / 2, [188, 0, 0, 128], 2);

    // A pixel just outside the rect should be clear (fragment discards).
    common::assert_pixel(&buf, w, 0, 0, [0, 0, 0, 0], 0);
}
