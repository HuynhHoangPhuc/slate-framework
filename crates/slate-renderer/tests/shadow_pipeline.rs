//! Headless integration tests for [`slate_renderer::ShadowPipeline`] (Phase 6).
//!
//! Covers:
//!   - Single shadow renders non-zero alpha at center and near-zero at ±3σ edge.
//!   - Sigma sweep ({2, 4, 8, 16, 32}) produces non-clipped output.
//!   - Rounded-corner shadow produces falloff at corners.
//!   - Empty instance list produces no draw (noop).
//!   - Instance buffer capacity grows monotonically.

mod common;

use slate_renderer::{
    ShadowInstance, ShadowPipeline, ViewportUniform, create_unit_quad,
    viewport_bind_group_layout,
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

fn render_shadow(
    device: &Device,
    queue: &Queue,
    format: TextureFormat,
    w: u32,
    h: u32,
    instances: &[ShadowInstance],
) -> Vec<u8> {
    let (bgl, viewport_bg, _buf) = make_viewport(device, queue, w, h);
    let unit_quad = create_unit_quad(device);
    let mut pipeline = ShadowPipeline::new(device, format, &bgl);

    pipeline.prepare(device, queue, instances);

    common::render_to_texture(device, queue, format, w, h, |encoder, view| {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("shadow-test-pass"),
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
            multiview_mask: None::<std::num::NonZeroU32>,
        });
        pipeline.record(&mut pass, &viewport_bg, &unit_quad, 0..instances.len() as u32);
    })
}

fn pixel_alpha(buf: &[u8], w: u32, x: u32, y: u32) -> u8 {
    let off = (y as usize * w as usize + x as usize) * 4;
    buf[off + 3]
}

#[test]
fn shadow_center_has_high_alpha_edge_has_low() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("shadow_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Rgba8Unorm;
    let (w, h) = (128, 128);
    let sigma = 8.0f32;

    let instances = [ShadowInstance {
        rect: [32.0, 32.0, 64.0, 64.0],
        color: [0.0, 0.0, 0.0, 1.0],
        corner_radius: 0.0,
        blur_radius: sigma,
        _pad: [0.0; 2],
    }];

    let buf = render_shadow(&device, &queue, format, w, h, &instances);

    // Center of the rect (64, 64) should have high alpha (close to 255).
    let center_a = pixel_alpha(&buf, w, 64, 64);
    assert!(
        center_a > 240,
        "center alpha should be near 255, got {center_a}"
    );

    // At ±3σ from rect edge (32 - 24 = 8, or 96 + 24 = 120), alpha should be near 0.
    let edge_a = pixel_alpha(&buf, w, 4, 64);
    assert!(
        edge_a < 15,
        "far edge alpha should be near 0, got {edge_a}"
    );
}

#[test]
fn sigma_sweep_no_clipping() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("shadow_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Rgba8Unorm;

    for &sigma in &[2.0f32, 4.0, 8.0, 16.0, 32.0] {
        // Rect size scales with sigma so the center always converges to high alpha.
        // A rect of 8σ×8σ ensures ample coverage for 4-tap integration.
        let rect_size = (8.0 * sigma).max(50.0);
        let rect_half = rect_size / 2.0;
        // Viewport large enough for the rect + ±3σ inflation + margin.
        let sz = (rect_size + 6.0 * sigma + 32.0).ceil() as u32;
        let center = sz as f32 / 2.0;

        let instances = [ShadowInstance {
            rect: [center - rect_half, center - rect_half, rect_size, rect_size],
            color: [0.0, 0.0, 0.0, 1.0],
            corner_radius: 0.0,
            blur_radius: sigma,
            _pad: [0.0; 2],
        }];

        let buf = render_shadow(&device, &queue, format, sz, sz, &instances);

        // Center should have high alpha (well inside the rect).
        let c = pixel_alpha(&buf, sz, sz / 2, sz / 2);
        assert!(
            c >= 190,
            "sigma={sigma}: center alpha too low ({c}), possible clipping"
        );

        // Corner of the viewport (0,0) should be nearly transparent (tail not clipped).
        let corner = pixel_alpha(&buf, sz, 0, 0);
        assert!(
            corner < 20,
            "sigma={sigma}: corner alpha too high ({corner}), tail possibly clipped"
        );
    }
}

#[test]
fn rounded_corner_shadow_falloff() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("shadow_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Rgba8Unorm;
    let (w, h) = (128, 128);
    let sigma = 8.0f32;
    let corner_radius = 20.0f32;

    let instances = [ShadowInstance {
        rect: [32.0, 32.0, 64.0, 64.0],
        color: [0.0, 0.0, 0.0, 1.0],
        corner_radius,
        blur_radius: sigma,
        _pad: [0.0; 2],
    }];

    let buf = render_shadow(&device, &queue, format, w, h, &instances);

    // Center should still be high alpha.
    let center_a = pixel_alpha(&buf, w, 64, 64);
    assert!(
        center_a > 240,
        "rounded shadow center alpha should be high, got {center_a}"
    );

    // Corner of the rect (32, 32) is inside the rounded corner zone — with
    // corner_radius=20, the top-left corner is clipped. Alpha at the very
    // corner of the bounding box should be lower than center.
    let corner_a = pixel_alpha(&buf, w, 32, 32);
    assert!(
        corner_a < center_a,
        "corner alpha ({corner_a}) should be less than center ({center_a}) due to rounding"
    );
}

#[test]
fn empty_instances_noop() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("shadow_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Rgba8Unorm;
    let (w, h) = (32, 32);

    let buf = render_shadow(&device, &queue, format, w, h, &[]);

    // All pixels should be transparent (clear color).
    for px in buf.chunks_exact(4) {
        assert_eq!(px, [0, 0, 0, 0]);
    }
}

#[test]
fn capacity_grows_monotonically() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("shadow_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Rgba8Unorm;
    let bgl = viewport_bind_group_layout(&device);
    let mut pipeline = ShadowPipeline::new(&device, format, &bgl);

    let shadow = ShadowInstance {
        rect: [0.0, 0.0, 10.0, 10.0],
        color: [0.0, 0.0, 0.0, 1.0],
        corner_radius: 0.0,
        blur_radius: 4.0,
        _pad: [0.0; 2],
    };

    // Small batch.
    let small: Vec<_> = vec![shadow; 10];
    pipeline.prepare(&device, &queue, &small);
    let cap_small = pipeline.capacity_bytes();

    // Large batch forces growth.
    let large: Vec<_> = vec![shadow; 200];
    pipeline.prepare(&device, &queue, &large);
    let cap_large = pipeline.capacity_bytes();
    assert!(cap_large > cap_small);

    // Smaller batch doesn't shrink.
    pipeline.prepare(&device, &queue, &small);
    assert_eq!(pipeline.capacity_bytes(), cap_large);
}
