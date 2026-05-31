//! GATE test for the P2 renderer clip (wgpu scissor) addition.
//!
//! This is the go/no-go for P2: before any ScrollView / scrollbar /
//! virtualization work, the renderer must be able to mask a layer's draws to
//! an axis-aligned rectangle, compose that across the live pipelines, and nest
//! (clip-inside-clip via intersection). Two levels, mirroring
//! `layer_ordering_walk.rs`:
//!
//!   1. Pure (no-GPU) assertions on the clip math: `ClipRect::intersect`
//!      (the nesting primitive) and `ClipRect::to_scissor_px` (logical→
//!      physical scissor conversion, scaling, clamping, off-screen rejection).
//!   2. Headless pixel tests that drive a clipped rect through the same
//!      "one scissor per layer" loop the production submit path uses, and
//!      assert that pixels outside the clip fall through to the layer beneath.
//!
//! Coordinate note: the offscreen target is rendered at scale 1.0, so logical
//! and physical pixels coincide here — the scaling path is covered by the unit
//! tests above.

mod common;

use slate_renderer::{
    ClipRect, InstancedRectPipeline, Layer, Lpx, RectInstance, ViewportUniform, create_unit_quad,
    srgb_u8_to_linear_premul, viewport_bind_group_layout,
};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindingResource, Buffer,
    BufferDescriptor, BufferUsages, Color, Device, LoadOp, Operations, Queue,
    RenderPassColorAttachment, RenderPassDescriptor, StoreOp, TextureFormat,
};

// ---------------------------------------------------------------------------
// Pure clip-math (no GPU)
// ---------------------------------------------------------------------------

#[test]
fn intersect_overlapping_region() {
    let a = ClipRect::new(Lpx(0.0), Lpx(0.0), Lpx(10.0), Lpx(10.0));
    let b = ClipRect::new(Lpx(5.0), Lpx(5.0), Lpx(10.0), Lpx(10.0));
    assert_eq!(
        a.intersect(&b),
        ClipRect::new(Lpx(5.0), Lpx(5.0), Lpx(5.0), Lpx(5.0))
    );
    // Intersection is commutative.
    assert_eq!(a.intersect(&b), b.intersect(&a));
}

#[test]
fn intersect_disjoint_is_zero_area() {
    let a = ClipRect::new(Lpx(0.0), Lpx(0.0), Lpx(10.0), Lpx(10.0));
    let b = ClipRect::new(Lpx(20.0), Lpx(20.0), Lpx(5.0), Lpx(5.0));
    let c = a.intersect(&b);
    assert_eq!(c.w, Lpx(0.0));
    assert_eq!(c.h, Lpx(0.0));
}

#[test]
fn to_scissor_px_scales_and_rounds() {
    let c = ClipRect::new(Lpx(10.0), Lpx(5.0), Lpx(20.0), Lpx(10.0));
    // scale 2.0: x*2=20, y*2=10, right=(30)*2=60 → w=40, bottom=(15)*2=30 → h=20.
    assert_eq!(c.to_scissor_px(2.0, 1000, 1000), Some((20, 10, 40, 20)));
}

#[test]
fn to_scissor_px_clamps_to_target() {
    // Spills past every edge; clamps to the attachment.
    let c = ClipRect::new(Lpx(-5.0), Lpx(-5.0), Lpx(100.0), Lpx(100.0));
    assert_eq!(c.to_scissor_px(1.0, 50, 40), Some((0, 0, 50, 40)));
}

#[test]
fn to_scissor_px_offscreen_or_empty_is_none() {
    // Fully off-screen → None (caller skips the layer).
    let off = ClipRect::new(Lpx(100.0), Lpx(100.0), Lpx(10.0), Lpx(10.0));
    assert_eq!(off.to_scissor_px(1.0, 50, 50), None);
    // Zero-area → None.
    let empty = ClipRect::new(Lpx(10.0), Lpx(10.0), Lpx(0.0), Lpx(0.0));
    assert_eq!(empty.to_scissor_px(1.0, 50, 50), None);
}

// ---------------------------------------------------------------------------
// Headless pixel tests
// ---------------------------------------------------------------------------

fn make_viewport(
    device: &Device,
    queue: &Queue,
    w: u32,
    h: u32,
) -> (BindGroupLayout, BindGroup, Buffer) {
    let bgl = viewport_bind_group_layout(device);
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("clip-test-viewport-buf"),
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
        label: Some("clip-test-viewport-bg"),
        layout: &bgl,
        entries: &[BindGroupEntry {
            binding: 0,
            resource: BindingResource::Buffer(buffer.as_entire_buffer_binding()),
        }],
    });
    (bgl, bind_group, buffer)
}

fn full_rect(w: u32, h: u32, rgba: [u8; 4]) -> RectInstance {
    RectInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        color: srgb_u8_to_linear_premul(rgba),
        corner_radius: Lpx(0.0),
        _pad: [0.0; 3],
    }
}

/// Render `layers` (indexing into `rects`) through the production-style
/// per-layer scissor loop at scale 1.0, returning the RGBA readback.
fn render_clipped(
    device: &Device,
    queue: &Queue,
    w: u32,
    h: u32,
    rects: &[RectInstance],
    layers: &[Layer],
) -> Vec<u8> {
    let format = TextureFormat::Bgra8UnormSrgb;
    let (bgl, viewport_bg, _vp) = make_viewport(device, queue, w, h);
    let unit_quad = create_unit_quad(device);
    let mut rect_pipeline = InstancedRectPipeline::new(device, format, &bgl);
    rect_pipeline.prepare(device, queue, rects);

    common::render_to_texture(device, queue, format, w, h, |encoder, view| {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("clip-walk-pass"),
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

        // Same shape as `renderer/submit.rs`: one scissor update per layer.
        for layer in layers {
            match layer.clip {
                Some(clip) => match clip.to_scissor_px(1.0, w, h) {
                    Some((x, y, sw, sh)) => pass.set_scissor_rect(x, y, sw, sh),
                    None => continue,
                },
                None => pass.set_scissor_rect(0, 0, w, h),
            }
            rect_pipeline.record(&mut pass, &viewport_bg, &unit_quad, layer.rects.clone());
        }
    })
}

#[test]
fn clip_masks_layer_to_scissor_rect() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("clip_walk: no GPU adapter — skipping");
        return;
    };
    let (w, h) = (64u32, 32u32);

    // Layer 0: full-viewport red, unclipped. Layer 1: full-viewport blue,
    // clipped to a vertical band x ∈ [16, 32). Blue must appear ONLY in the
    // band; red shows through everywhere else.
    let rects = vec![full_rect(w, h, [255, 0, 0, 255]), full_rect(w, h, [0, 0, 255, 255])];
    let band = ClipRect::new(Lpx(16.0), Lpx(0.0), Lpx(16.0), Lpx(32.0));
    let layers = [
        Layer {
            rects: 0..1,
            shadows: 0..0,
            images: 0..0,
            glyphs: 0..0,
            clip: None,
            depth: 0,
        },
        Layer {
            rects: 1..2,
            shadows: 0..0,
            images: 0..0,
            glyphs: 0..0,
            clip: Some(band),
            depth: 0,
        },
    ];

    let buf = render_clipped(&device, &queue, w, h, &rects, &layers);

    // Left of band → red (blue clipped out).
    common::assert_pixel(&buf, w, 4, 16, [255, 0, 0, 255], 2);
    // Inside band → blue.
    common::assert_pixel(&buf, w, 24, 16, [0, 0, 255, 255], 2);
    // Right of band → red.
    common::assert_pixel(&buf, w, 48, 16, [255, 0, 0, 255], 2);
}

#[test]
fn nested_clip_intersects() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("clip_walk: no GPU adapter — skipping");
        return;
    };
    let (w, h) = (64u32, 32u32);

    // Blue clipped to the intersection of a vertical band x ∈ [16, 48) and a
    // horizontal band y ∈ [8, 24) — i.e. a centered window. This is the
    // nesting case: scroll-inside-scroll yields `parent.intersect(child)`.
    let rects = vec![full_rect(w, h, [255, 0, 0, 255]), full_rect(w, h, [0, 0, 255, 255])];
    let vert = ClipRect::new(Lpx(16.0), Lpx(0.0), Lpx(32.0), Lpx(32.0));
    let horiz = ClipRect::new(Lpx(0.0), Lpx(8.0), Lpx(64.0), Lpx(16.0));
    let window = vert.intersect(&horiz);
    let layers = [
        Layer {
            rects: 0..1,
            shadows: 0..0,
            images: 0..0,
            glyphs: 0..0,
            clip: None,
            depth: 0,
        },
        Layer {
            rects: 1..2,
            shadows: 0..0,
            images: 0..0,
            glyphs: 0..0,
            clip: Some(window),
            depth: 0,
        },
    ];

    let buf = render_clipped(&device, &queue, w, h, &rects, &layers);

    // Center of the window → blue.
    common::assert_pixel(&buf, w, 32, 16, [0, 0, 255, 255], 2);
    // Above the horizontal band (but inside the vertical one) → red.
    common::assert_pixel(&buf, w, 32, 2, [255, 0, 0, 255], 2);
    // Left of the vertical band (but inside the horizontal one) → red.
    common::assert_pixel(&buf, w, 4, 16, [255, 0, 0, 255], 2);
}
