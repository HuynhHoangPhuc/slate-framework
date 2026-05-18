//! End-to-end smoke test for the premultiplied-alpha contract.
//!
//! Renders a single rect via [`InstancedRectPipeline`] into a 32×32 off-screen
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

use slate_renderer::{
    InstancedRectPipeline, Lpx, RectInstance, ViewportUniform, create_unit_quad,
    srgb_u8_to_linear_premul, viewport_bind_group_layout,
};
use wgpu::{
    BindGroupDescriptor, BindGroupEntry, BindingResource, BufferDescriptor, BufferUsages, Color,
    LoadOp, Operations, RenderPassColorAttachment, RenderPassDescriptor, StoreOp, TextureFormat,
};

#[test]
fn premul_red_half_alpha_round_trips_through_pipeline() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("smoke_premul: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);

    let bgl = viewport_bind_group_layout(&device);
    let viewport_buf = device.create_buffer(&BufferDescriptor {
        label: Some("smoke-viewport-buf"),
        size: std::mem::size_of::<ViewportUniform>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &viewport_buf,
        0,
        bytemuck::bytes_of(&ViewportUniform {
            size: [Lpx(w as f32), Lpx(h as f32)],
            _pad: [0.0; 2],
        }),
    );
    let viewport_bg = device.create_bind_group(&BindGroupDescriptor {
        label: Some("smoke-viewport-bg"),
        layout: &bgl,
        entries: &[BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(viewport_buf.as_entire_buffer_binding()),
        }],
    });
    let unit_quad = create_unit_quad(&device);
    let mut pipeline = InstancedRectPipeline::new(&device, format, &bgl);

    let instances = [RectInstance {
        rect: [Lpx(4.0), Lpx(4.0), Lpx(24.0), Lpx(24.0)],
        color: srgb_u8_to_linear_premul([255, 0, 0, 128]),
        corner_radius: Lpx(0.0),
        _pad: [0.0; 3],
    }];
    pipeline.prepare(&device, &queue, &instances);

    let buf = common::render_to_texture(&device, &queue, format, w, h, |encoder, view| {
        let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("smoke-pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(Color::TRANSPARENT),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        pipeline.record(&mut rpass, &viewport_bg, &unit_quad, 0..1);
    });

    // Center pixel — well inside the rect, coverage = 1.0.
    common::assert_pixel(&buf, w, w / 2, h / 2, [188, 0, 0, 128], 2);

    // A pixel just outside the rect should be clear (fragment discards).
    common::assert_pixel(&buf, w, 0, 0, [0, 0, 0, 0], 0);
}
