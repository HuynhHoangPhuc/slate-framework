//! Headless integration tests for [`slate_renderer::InstancedRectPipeline`]
//! (Phase 2).
//!
//! Covers:
//!   - 100 rects rendered in one `draw` call produce expected center-pixel
//!     colors at multiple positions.
//!   - Instance buffer grows monotonically across `prepare` calls and never
//!     shrinks below high-water mark.
//!   - The two-phase `prepare → begin_render_pass → record` sequence
//!     compiles and runs cleanly (red-team P0-2 regression check — covered
//!     implicitly by the first test).

mod common;

use slate_renderer::{
    InstancedRectPipeline, RectInstance, ViewportUniform, create_unit_quad,
    srgb_u8_to_linear_premul, viewport_bind_group_layout,
};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindingResource, Buffer,
    BufferDescriptor, BufferUsages, Color, Device, LoadOp, Operations, Queue,
    RenderPassColorAttachment, RenderPassDescriptor, StoreOp, TextureFormat,
};

fn make_viewport(
    device: &Device,
    queue: &Queue,
    w: u32,
    h: u32,
) -> (BindGroupLayout, BindGroup, Buffer) {
    let bgl = viewport_bind_group_layout(device);
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("test-viewport-buf"),
        size: std::mem::size_of::<ViewportUniform>() as u64,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &buffer,
        0,
        bytemuck::bytes_of(&ViewportUniform {
            size: [w as f32, h as f32],
            _pad: [0.0; 2],
        }),
    );
    let bind_group = device.create_bind_group(&BindGroupDescriptor {
        label: Some("test-viewport-bg"),
        layout: &bgl,
        entries: &[BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(buffer.as_entire_buffer_binding()),
        }],
    });
    (bgl, bind_group, buffer)
}

#[test]
fn instanced_pipeline_renders_100_rects_in_one_draw() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("instanced_rect: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (100u32, 100u32);
    let (bgl, viewport_bg, _viewport_buf) = make_viewport(&device, &queue, w, h);
    let unit_quad = create_unit_quad(&device);
    let mut pipeline = InstancedRectPipeline::new(&device, format, &bgl);

    // 10×10 grid of 8×8 rects on a 10-px pitch — leaves 2-px gaps between rects.
    let white = srgb_u8_to_linear_premul([255, 255, 255, 255]);
    let mut instances = Vec::with_capacity(100);
    for j in 0..10 {
        for i in 0..10 {
            instances.push(RectInstance {
                rect: [i as f32 * 10.0, j as f32 * 10.0, 8.0, 8.0],
                color: white,
                corner_radius: 0.0,
                _pad: [0.0; 3],
            });
        }
    }
    pipeline.prepare(&device, &queue, &instances);

    let buf = common::render_to_texture(&device, &queue, format, w, h, |encoder, view| {
        let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("instanced-rect-test-pass"),
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
        pipeline.record(
            &mut rpass,
            &viewport_bg,
            &unit_quad,
            0..(instances.len() as u32),
        );
    });

    // Centers of rect[0,0] and rect[5,5] should be opaque white.
    common::assert_pixel(&buf, w, 4, 4, [255, 255, 255, 255], 2);
    common::assert_pixel(&buf, w, 54, 54, [255, 255, 255, 255], 2);
    common::assert_pixel(&buf, w, 94, 94, [255, 255, 255, 255], 2);

    // Pixel in the 2-px gap between rects (rect[0,0] ends at x=8; rect[1,0]
    // starts at x=10) should be untouched (clear color = transparent).
    common::assert_pixel(&buf, w, 9, 4, [0, 0, 0, 0], 2);
    common::assert_pixel(&buf, w, 9, 9, [0, 0, 0, 0], 2);
}

#[test]
fn capacity_grows_monotonically() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("instanced_rect: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let bgl = viewport_bind_group_layout(&device);
    let mut pipeline = InstancedRectPipeline::new(&device, format, &bgl);

    let stride = std::mem::size_of::<RectInstance>() as u64;
    let initial = pipeline.capacity_bytes();
    assert!(
        initial >= 64 * stride,
        "default capacity {initial} below 64 × {stride} instances"
    );

    let dummy = RectInstance {
        rect: [0.0, 0.0, 1.0, 1.0],
        color: [1.0; 4],
        corner_radius: 0.0,
        _pad: [0.0; 3],
    };

    // Fits in initial capacity — no growth.
    pipeline.prepare(&device, &queue, &vec![dummy; 64]);
    assert_eq!(pipeline.capacity_bytes(), initial, "grew unnecessarily");

    // 200 instances forces growth to next_power_of_two(200 × stride).
    pipeline.prepare(&device, &queue, &vec![dummy; 200]);
    assert!(
        pipeline.capacity_bytes() >= 256 * stride,
        "capacity {} did not grow to fit 200 instances",
        pipeline.capacity_bytes()
    );

    // Smaller subsequent prepare must NOT shrink.
    let high_water = pipeline.capacity_bytes();
    pipeline.prepare(&device, &queue, &vec![dummy; 32]);
    assert_eq!(
        pipeline.capacity_bytes(),
        high_water,
        "capacity shrunk on smaller prepare"
    );
}

#[test]
fn empty_instances_is_a_noop() {
    let Some((device, queue)) = common::make_headless_device() else {
        return;
    };
    let format = TextureFormat::Bgra8UnormSrgb;
    let bgl = viewport_bind_group_layout(&device);
    let mut pipeline = InstancedRectPipeline::new(&device, format, &bgl);
    let before = pipeline.capacity_bytes();
    pipeline.prepare(&device, &queue, &[]);
    assert_eq!(pipeline.capacity_bytes(), before);
}
