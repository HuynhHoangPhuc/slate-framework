//! Regression lock for the z-order seam (`LayerOrdering` / `DefaultPainterOrder`).
//!
//! No renderer-level golden images existed for the per-frame layer walk, so
//! this establishes the byte-comparison baseline that the trait extraction must
//! preserve. Two levels:
//!
//!   1. A pure (no-GPU) assertion that `DefaultPainterOrder` emits exactly the
//!      v1 sequence: layers in push order, and within each layer the fixed
//!      shadows → rects → glyphs → images order.
//!   2. A headless pixel test that drives an overlapping rect + image across two
//!      layers *through the trait* and asserts the stacking winner — locking
//!      both the layer order (later layer wins) and the within-layer primitive
//!      order (image draws over rect).

mod common;

use slate_renderer::atlas::{Atlas, Format};
use slate_renderer::scene::Layer;
use slate_renderer::{
    DefaultPainterOrder, ImageInstance, ImagePipeline, InstancedRectPipeline, LayerOrdering, Lpx,
    RectInstance, ScenePrimitive, ViewportUniform, create_unit_quad, srgb_u8_to_linear_premul,
    viewport_bind_group_layout,
};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindingResource, Buffer,
    BufferDescriptor, BufferUsages, Color, Device, LoadOp, Operations, Queue,
    RenderPassColorAttachment, RenderPassDescriptor, StoreOp, TextureFormat,
};

#[test]
fn default_painter_order_emits_v1_sequence() {
    // The contract the renderer's pass-recording loop relies on: for each layer
    // in push order, the four primitive kinds in shadows → rects → glyphs →
    // images order. Reordering this is a silent draw-order regression.
    let mut seq = Vec::new();
    DefaultPainterOrder.for_each_draw(2, |layer, kind| seq.push((layer, kind)));

    assert_eq!(
        seq,
        vec![
            (0, ScenePrimitive::Shadow),
            (0, ScenePrimitive::Rect),
            (0, ScenePrimitive::Glyph),
            (0, ScenePrimitive::Image),
            (1, ScenePrimitive::Shadow),
            (1, ScenePrimitive::Rect),
            (1, ScenePrimitive::Glyph),
            (1, ScenePrimitive::Image),
        ]
    );
}

#[test]
fn zero_layers_emits_nothing() {
    let mut count = 0;
    DefaultPainterOrder.for_each_draw(0, |_, _| count += 1);
    assert_eq!(count, 0);
}

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
fn multi_layer_overlap_resolves_via_painter_order() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("layer_ordering_walk: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (64u32, 32u32);
    let (bgl, viewport_bg, _vp_buf) = make_viewport(&device, &queue, w, h);
    let unit_quad = create_unit_quad(&device);

    // Atlas patches sampled by the image instances.
    let mut atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);
    let blue: Vec<u8> = (0..16).flat_map(|_| [0u8, 0, 255, 255]).collect();
    let green: Vec<u8> = (0..16).flat_map(|_| [0u8, 255, 0, 255]).collect();
    let blue_alloc = atlas.allocate(4, 4).expect("blue patch");
    atlas.upload(&queue, blue_alloc.alloc_id, &blue);
    let green_alloc = atlas.allocate(4, 4).expect("green patch");
    atlas.upload(&queue, green_alloc.alloc_id, &green);

    let mut rect_pipeline = InstancedRectPipeline::new(&device, format, &bgl);
    let mut image_pipeline = ImagePipeline::new(&device, format, &bgl, &atlas);

    // Layer 0: a full-viewport red rect, then a full-viewport blue image.
    //   → within the layer, the image draws over the rect (blue beats red).
    // Layer 1: a green image over the LEFT half only.
    //   → the later layer beats layer 0 there (green beats blue on the left).
    let rects = vec![RectInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        color: srgb_u8_to_linear_premul([255, 0, 0, 255]),
        corner_radius: Lpx(0.0),
        _pad: [0.0; 3],
    }];
    let images = vec![
        // layer 0 image: blue, full viewport.
        ImageInstance {
            rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
            uv_rect: blue_alloc.uv_rect,
            tint: [1.0; 4],
        },
        // layer 1 image: green, left half only.
        ImageInstance {
            rect: [Lpx(0.0), Lpx(0.0), Lpx(32.0), Lpx(h as f32)],
            uv_rect: green_alloc.uv_rect,
            tint: [1.0; 4],
        },
    ];

    let layers = [
        Layer {
            rects: 0..1,
            shadows: 0..0,
            images: 0..1,
            glyphs: 0..0,
        },
        Layer {
            rects: 1..1,
            shadows: 0..0,
            images: 1..2,
            glyphs: 0..0,
        },
    ];

    rect_pipeline.prepare(&device, &queue, &rects);
    image_pipeline.prepare(&device, &queue, &images);

    let buf = common::render_to_texture(&device, &queue, format, w, h, |encoder, view| {
        let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("layer-ordering-walk-pass"),
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

        // Drive the walk through the trait, exactly as the renderer does. Shadow
        // and glyph ranges are empty here, so those steps are no-ops.
        DefaultPainterOrder.for_each_draw(layers.len(), |idx, kind| {
            let layer = &layers[idx];
            match kind {
                ScenePrimitive::Shadow | ScenePrimitive::Glyph => {}
                ScenePrimitive::Rect => {
                    rect_pipeline.record(&mut pass, &viewport_bg, &unit_quad, layer.rects.clone());
                }
                ScenePrimitive::Image => {
                    image_pipeline.record(&mut pass, &viewport_bg, &unit_quad, layer.images.clone());
                }
            }
        });
    });
    drop(atlas);

    // Left half: layer-1 green wins over layer-0 blue (layer order).
    common::assert_pixel(&buf, w, 16, 16, [0, 255, 0, 255], 2);
    // Right half: layer-0 blue image wins over layer-0 red rect (primitive order).
    common::assert_pixel(&buf, w, 48, 16, [0, 0, 255, 255], 2);
}
