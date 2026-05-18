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
    InstancedRectPipeline, Lpx, RectInstance, ViewportUniform, create_unit_quad,
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
            size: [Lpx(w as f32), Lpx(h as f32)],
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
                rect: [
                    Lpx(i as f32 * 10.0),
                    Lpx(j as f32 * 10.0),
                    Lpx(8.0),
                    Lpx(8.0),
                ],
                color: white,
                corner_radius: Lpx(0.0),
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
        rect: [Lpx(0.0), Lpx(0.0), Lpx(1.0), Lpx(1.0)],
        color: [1.0; 4],
        corner_radius: Lpx(0.0),
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

#[test]
fn multi_layer_disjoint_ranges_in_single_pass() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("instanced_rect: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (100u32, 100u32);
    let (bgl, viewport_bg, _viewport_buf) = make_viewport(&device, &queue, w, h);
    let unit_quad = create_unit_quad(&device);
    let mut pipeline = InstancedRectPipeline::new(&device, format, &bgl);

    // Prepare 5 rects: 2 in "layer 0" (indices 0..2), 3 in "layer 1" (indices 2..5).
    let white = srgb_u8_to_linear_premul([255, 255, 255, 255]);
    let red = srgb_u8_to_linear_premul([255, 0, 0, 255]);
    let instances = vec![
        // Layer 0 (indices 0..2): white rects
        RectInstance {
            rect: [Lpx(10.0), Lpx(10.0), Lpx(8.0), Lpx(8.0)],
            color: white,
            corner_radius: Lpx(0.0),
            _pad: [0.0; 3],
        },
        RectInstance {
            rect: [Lpx(30.0), Lpx(10.0), Lpx(8.0), Lpx(8.0)],
            color: white,
            corner_radius: Lpx(0.0),
            _pad: [0.0; 3],
        },
        // Layer 1 (indices 2..5): red rects
        RectInstance {
            rect: [Lpx(50.0), Lpx(10.0), Lpx(8.0), Lpx(8.0)],
            color: red,
            corner_radius: Lpx(0.0),
            _pad: [0.0; 3],
        },
        RectInstance {
            rect: [Lpx(70.0), Lpx(10.0), Lpx(8.0), Lpx(8.0)],
            color: red,
            corner_radius: Lpx(0.0),
            _pad: [0.0; 3],
        },
        RectInstance {
            rect: [Lpx(10.0), Lpx(30.0), Lpx(8.0), Lpx(8.0)],
            color: red,
            corner_radius: Lpx(0.0),
            _pad: [0.0; 3],
        },
    ];
    pipeline.prepare(&device, &queue, &instances);

    let buf = common::render_to_texture(&device, &queue, format, w, h, |encoder, view| {
        let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("multi-layer-test-pass"),
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
        // Record layer 0 (white): range 0..2
        pipeline.record(&mut rpass, &viewport_bg, &unit_quad, 0..2);
        // Record layer 1 (red): range 2..5
        pipeline.record(&mut rpass, &viewport_bg, &unit_quad, 2..5);
    });

    // Centers of white rects (layer 0) at (14, 14) and (34, 14) should be opaque white.
    common::assert_pixel(&buf, w, 14, 14, [255, 255, 255, 255], 2);
    common::assert_pixel(&buf, w, 34, 14, [255, 255, 255, 255], 2);

    // Centers of red rects (layer 1) at (54, 14), (74, 14), (14, 34) should be opaque red.
    common::assert_pixel(&buf, w, 54, 14, [255, 0, 0, 255], 2);
    common::assert_pixel(&buf, w, 74, 14, [255, 0, 0, 255], 2);
    common::assert_pixel(&buf, w, 14, 34, [255, 0, 0, 255], 2);
}

#[test]
fn premul_alpha_parity_with_single_instanced_rect() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("instanced_rect: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let (bgl, viewport_bg, _viewport_buf) = make_viewport(&device, &queue, w, h);
    let unit_quad = create_unit_quad(&device);
    let mut pipeline = InstancedRectPipeline::new(&device, format, &bgl);

    // Single instanced rect at center, same color as smoke_premul test:
    // sRGB (255, 0, 0, 128) → linear premul (0.502, 0, 0, 0.502)
    let instances = vec![RectInstance {
        rect: [
            Lpx(w as f32 / 2.0 - 8.0),
            Lpx(h as f32 / 2.0 - 8.0),
            Lpx(16.0),
            Lpx(16.0),
        ],
        color: srgb_u8_to_linear_premul([255, 0, 0, 128]),
        corner_radius: Lpx(0.0),
        _pad: [0.0; 3],
    }];
    pipeline.prepare(&device, &queue, &instances);

    let buf = common::render_to_texture(&device, &queue, format, w, h, |encoder, view| {
        let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("premul-parity-pass"),
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

    // Center pixel (16, 16) should match smoke_premul: ≈(188, 0, 0, 128) ±2/255.
    // (This is the sRGB-encoded result of linear premul (0.502, 0, 0, 0.502)
    // blended over transparent with One/OneMinusSrcAlpha blend state.)
    common::assert_pixel(&buf, w, w / 2, h / 2, [188, 0, 0, 128], 2);

    // Corner pixel (0, 0) should be clear (outside the rect).
    common::assert_pixel(&buf, w, 0, 0, [0, 0, 0, 0], 0);
}

#[test]
fn corner_radius_produces_rounded_corners() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("instanced_rect: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (20u32, 20u32);
    let (bgl, viewport_bg, _viewport_buf) = make_viewport(&device, &queue, w, h);
    let unit_quad = create_unit_quad(&device);
    let mut pipeline = InstancedRectPipeline::new(&device, format, &bgl);

    // Single 16×16 rect at (2, 2) with corner_radius=8 (covers entire square).
    // The SDF with radius=8 on a 16×16 rect means the rect occupies [0..16, 0..16]
    // with center at (8, 8); radius-8 corners are fully rounded.
    let white = srgb_u8_to_linear_premul([255, 255, 255, 255]);
    let instances = vec![RectInstance {
        rect: [Lpx(2.0), Lpx(2.0), Lpx(16.0), Lpx(16.0)],
        color: white,
        corner_radius: Lpx(8.0),
        _pad: [0.0; 3],
    }];
    pipeline.prepare(&device, &queue, &instances);

    let buf = common::render_to_texture(&device, &queue, format, w, h, |encoder, view| {
        let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("corner-radius-pass"),
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

    // Center pixel (10, 10) should be opaque (well inside the rounded rect).
    common::assert_pixel(&buf, w, 10, 10, [255, 255, 255, 255], 2);

    // The four corners (e.g., pixel 2,2; 17,2; 2,17; 17,17) should be
    // mostly discarded or very faint. With radius=8 on a 16×16 rect, the
    // SDF at the corners is positive (outside), so coverage is near 0.
    // Allow some AA bleed (tolerance=2), but expect mostly transparent.
    common::assert_pixel(&buf, w, 2, 2, [0, 0, 0, 0], 1);
    common::assert_pixel(&buf, w, 17, 2, [0, 0, 0, 0], 1);
    common::assert_pixel(&buf, w, 2, 17, [0, 0, 0, 0], 1);
    common::assert_pixel(&buf, w, 17, 17, [0, 0, 0, 0], 1);
}

#[test]
fn empty_range_in_record_is_safe() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("instanced_rect: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (100u32, 100u32);
    let (bgl, viewport_bg, _viewport_buf) = make_viewport(&device, &queue, w, h);
    let unit_quad = create_unit_quad(&device);
    let mut pipeline = InstancedRectPipeline::new(&device, format, &bgl);

    // Prepare with one rect.
    let white = srgb_u8_to_linear_premul([255, 255, 255, 255]);
    let instances = vec![RectInstance {
        rect: [Lpx(10.0), Lpx(10.0), Lpx(8.0), Lpx(8.0)],
        color: white,
        corner_radius: Lpx(0.0),
        _pad: [0.0; 3],
    }];
    pipeline.prepare(&device, &queue, &instances);

    let buf = common::render_to_texture(&device, &queue, format, w, h, |encoder, view| {
        let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("empty-range-test-pass"),
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
        // Record empty range — should not panic or crash.
        pipeline.record(&mut rpass, &viewport_bg, &unit_quad, 0..0);
        // Record non-empty range — should draw normally.
        pipeline.record(&mut rpass, &viewport_bg, &unit_quad, 0..1);
    });

    // Center of the rect should still be drawn (the non-empty range call took effect).
    common::assert_pixel(&buf, w, 14, 14, [255, 255, 255, 255], 2);
    // Clear area should remain clear.
    common::assert_pixel(&buf, w, 50, 50, [0, 0, 0, 0], 0);
}
