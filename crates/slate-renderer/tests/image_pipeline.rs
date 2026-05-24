//! Headless integration tests for [`slate_renderer::ImagePipeline`].
//!
//! Covers:
//!   - sRGB roundtrip: 4×4 opaque-red atlas patch sampled by an instance
//!     covering the viewport renders red at the center pixel.
//!   - Tint multiplication: same patch with green premul tint renders black.
//!   - Premultiplied half-alpha: 4×4 straight-RGBA `(255, 0, 0, 128)` patch
//!     uploaded into `Rgba8UnormSrgb` atlas; white tint; expects sRGB-encoded
//!     readback `(188, 0, 0, 128)` ±2/255.

mod common;

use slate_renderer::atlas::{Atlas, Format, PAGE_SIZE};
use slate_renderer::{
    ImageInstance, ImagePipeline, Lpx, ViewportUniform, allocate_image, create_unit_quad,
    linear_to_srgb_channel, pad_rgba_with_gutter, srgb_u8_to_linear_premul,
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

/// Allocate a `width × height` patch in `atlas`, upload `pixels`, return its
/// uv_rect for an `ImageInstance`.
fn upload_patch(
    atlas: &mut Atlas,
    queue: &Queue,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> [f32; 4] {
    let alloc = atlas.allocate(width, height).expect("atlas alloc");
    atlas.upload(queue, alloc.alloc_id, pixels);
    alloc.uv_rect
}

fn run_image_test(
    w: u32,
    h: u32,
    instances: &[ImageInstance],
    device: &Device,
    queue: &Queue,
    pipeline: &mut ImagePipeline,
    label: &'static str,
) -> Vec<u8> {
    let format = TextureFormat::Bgra8UnormSrgb;
    let (_bgl, viewport_bg, _vp_buf) = make_viewport(device, queue, w, h);
    let unit_quad = create_unit_quad(device);
    pipeline.prepare(device, queue, instances);

    common::render_to_texture(device, queue, format, w, h, |encoder, view| {
        let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some(label),
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
    })
}

#[test]
fn opaque_red_patch_renders_red() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("image_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);

    // 4×4 opaque red patch (straight RGBA, sRGB-encoded bytes).
    let red_pixels: Vec<u8> = (0..16).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let uv_rect = upload_patch(&mut atlas, &queue, 4, 4, &red_pixels);

    let mut pipeline = ImagePipeline::new(&device, format, &bgl, &atlas);

    let instances = vec![ImageInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        uv_rect,
        tint: [1.0, 1.0, 1.0, 1.0],
    }];

    let buf = run_image_test(
        w,
        h,
        &instances,
        &device,
        &queue,
        &mut pipeline,
        "image-red-pass",
    );
    // `atlas` is held by `make_atlas_bind_group` via Arc-clone of the
    // TextureView; no extra keep-alive needed.
    drop(atlas);

    common::assert_pixel(&buf, w, w / 2, h / 2, [255, 0, 0, 255], 2);
}

#[test]
fn green_tint_zeroes_red_patch() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("image_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);

    let red_pixels: Vec<u8> = (0..16).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let uv_rect = upload_patch(&mut atlas, &queue, 4, 4, &red_pixels);

    let mut pipeline = ImagePipeline::new(&device, format, &bgl, &atlas);

    // Premultiplied green tint: red × green = 0 in every channel.
    let green_tint = srgb_u8_to_linear_premul([0, 255, 0, 255]);
    let instances = vec![ImageInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        uv_rect,
        tint: green_tint,
    }];

    let buf = run_image_test(
        w,
        h,
        &instances,
        &device,
        &queue,
        &mut pipeline,
        "image-tint-pass",
    );
    drop(atlas);

    common::assert_pixel(&buf, w, w / 2, h / 2, [0, 0, 0, 255], 2);
}

#[test]
fn premul_half_alpha_red_patch_matches_smoke() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("image_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);

    // Straight RGBA (255, 0, 0, 128). Hardware decodes RGB sRGB→linear (1, 0, 0);
    // alpha is linear pass-through (≈0.502). Fragment premuls → (0.502, 0, 0, 0.502).
    // Surface is sRGB → linear_to_srgb(0.502) ≈ 188/255 on R; A passes through 128.
    let half_alpha: Vec<u8> = (0..16).flat_map(|_| [255u8, 0, 0, 128]).collect();
    let uv_rect = upload_patch(&mut atlas, &queue, 4, 4, &half_alpha);

    let mut pipeline = ImagePipeline::new(&device, format, &bgl, &atlas);

    let instances = vec![ImageInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        uv_rect,
        tint: [1.0, 1.0, 1.0, 1.0],
    }];

    let buf = run_image_test(
        w,
        h,
        &instances,
        &device,
        &queue,
        &mut pipeline,
        "image-half-alpha-pass",
    );
    drop(atlas);

    // Compute expected sRGB-encoded R from the actual color helpers instead
    // of hardcoding 187/188 — avoids drift if a future driver changes
    // rounding by 1 LSB or the helpers retune.
    let alpha_linear = 128_f32 / 255.0;
    let r_premul_linear = 1.0 * alpha_linear;
    let r_srgb = (linear_to_srgb_channel(r_premul_linear) * 255.0).round() as u8;
    common::assert_pixel(&buf, w, w / 2, h / 2, [r_srgb, 0, 0, 128], 2);
}

#[test]
fn rgba_channel_order_survives_atlas_and_surface_no_bgra_swap() {
    // Byte-order contract lock: an RGBA image uploaded into the Rgba8UnormSrgb
    // image atlas must reach the surface with channels in the SAME order — R
    // stays R, B stays B. A regression that uploads or samples as BGRA would
    // swap R↔B and read this patch mirrored. All four channels are distinct
    // (R≫B) so a swap fails loudly rather than aliasing.
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("image_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);

    // Straight, opaque sRGB bytes with strongly asymmetric channels.
    let (sr, sg, sb) = (200u8, 120u8, 40u8);
    let pixels: Vec<u8> = (0..16).flat_map(|_| [sr, sg, sb, 255]).collect();
    let uv_rect = upload_patch(&mut atlas, &queue, 4, 4, &pixels);

    let mut pipeline = ImagePipeline::new(&device, format, &bgl, &atlas);
    let instances = vec![ImageInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        uv_rect,
        tint: [1.0, 1.0, 1.0, 1.0],
    }];

    let buf = run_image_test(w, h, &instances, &device, &queue, &mut pipeline, "image-byte-order");
    drop(atlas);

    // Opaque + white tint: sRGB→linear on sample, premul by α=1, then
    // linear→sRGB on store round-trips back to the input sRGB bytes (±2 for
    // driver LSB rounding, matching the other roundtrip tests here).
    common::assert_pixel(&buf, w, w / 2, h / 2, [sr, sg, sb, 255], 2);
}

#[test]
fn capacity_grows_monotonically() {
    let Some((device, queue)) = common::make_headless_device() else {
        return;
    };
    let format = TextureFormat::Bgra8UnormSrgb;
    let bgl = viewport_bind_group_layout(&device);
    let atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);
    let mut pipeline = ImagePipeline::new(&device, format, &bgl, &atlas);

    let stride = std::mem::size_of::<ImageInstance>() as u64;
    let initial = pipeline.capacity_bytes();
    assert!(initial >= 64 * stride);

    let dummy = ImageInstance {
        rect: [Lpx(0.0); 4],
        uv_rect: [0.0, 0.0, 1.0, 1.0],
        tint: [1.0; 4],
    };

    pipeline.prepare(&device, &queue, &vec![dummy; 64]);
    assert_eq!(pipeline.capacity_bytes(), initial);

    pipeline.prepare(&device, &queue, &vec![dummy; 200]);
    assert!(pipeline.capacity_bytes() >= 256 * stride);

    let high_water = pipeline.capacity_bytes();
    pipeline.prepare(&device, &queue, &vec![dummy; 32]);
    assert_eq!(pipeline.capacity_bytes(), high_water);
}

#[test]
fn non_uniform_patch_preserves_orientation() {
    // A uniform 4×4 red patch can't detect a uv flip.
    // Top half red, bottom half blue → after sampling at top vs bottom of the
    // viewport-covering rect, top should read red, bottom should read blue.
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("image_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);

    // 4×4 patch: top 2 rows red, bottom 2 rows blue (PNG row-0 = top).
    let mut pixels: Vec<u8> = Vec::with_capacity(4 * 4 * 4);
    for y in 0..4 {
        for _x in 0..4 {
            if y < 2 {
                pixels.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                pixels.extend_from_slice(&[0, 0, 255, 255]);
            }
        }
    }
    let uv_rect = upload_patch(&mut atlas, &queue, 4, 4, &pixels);

    let mut pipeline = ImagePipeline::new(&device, format, &bgl, &atlas);

    let instances = vec![ImageInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        uv_rect,
        tint: [1.0, 1.0, 1.0, 1.0],
    }];

    let buf = run_image_test(
        w,
        h,
        &instances,
        &device,
        &queue,
        &mut pipeline,
        "image-orientation-pass",
    );
    drop(atlas);

    // Sample interior texel centers to avoid atlas-edge bleed (the gutter
    // added by allocate_image/allocate_glyph guards against this in production).
    // With a 32-px rect over a 4-row patch, texel centers land at viewport y ∈ {4,12,20,28}.
    // y=4 is row 0 (red, full); y=20 is the row-2/3 midpoint (both blue, full).
    common::assert_pixel(&buf, w, w / 2, 4, [255, 0, 0, 255], 2);
    common::assert_pixel(&buf, w, w / 2, 20, [0, 0, 255, 255], 2);
}

#[test]
fn multi_layer_disjoint_ranges_in_single_pass() {
    // Prepare all instances once, record disjoint sub-ranges for "layers" 0
    // and 1 inside a single pass.
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("image_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (64u32, 32u32);
    let (_bgl_v, viewport_bg, _vp_buf) = make_viewport(&device, &queue, w, h);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);

    let red: Vec<u8> = (0..16).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let green: Vec<u8> = (0..16).flat_map(|_| [0u8, 255, 0, 255]).collect();
    let red_uv = upload_patch(&mut atlas, &queue, 4, 4, &red);
    let green_uv = upload_patch(&mut atlas, &queue, 4, 4, &green);

    let mut pipeline = ImagePipeline::new(&device, format, &bgl, &atlas);
    let unit_quad = create_unit_quad(&device);

    // Layer 0: red on the left half. Layer 1: green on the right half.
    let instances = vec![
        ImageInstance {
            rect: [Lpx(0.0), Lpx(0.0), Lpx(32.0), Lpx(32.0)],
            uv_rect: red_uv,
            tint: [1.0; 4],
        },
        ImageInstance {
            rect: [Lpx(32.0), Lpx(0.0), Lpx(32.0), Lpx(32.0)],
            uv_rect: green_uv,
            tint: [1.0; 4],
        },
    ];
    pipeline.prepare(&device, &queue, &instances);

    let buf = common::render_to_texture(&device, &queue, format, w, h, |encoder, view| {
        let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("image-multi-layer-pass"),
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
        pipeline.record(&mut rpass, &viewport_bg, &unit_quad, 1..2);
    });
    drop(atlas);

    common::assert_pixel(&buf, w, 16, 16, [255, 0, 0, 255], 2);
    common::assert_pixel(&buf, w, 48, 16, [0, 255, 0, 255], 2);
}

#[test]
fn rebuild_atlas_bg_does_not_break_subsequent_draws() {
    // Smoke test: after rebuild_atlas_bg with the same texture view, the
    // pipeline still draws correctly. The renderer swaps views on atlas growth;
    // this guards the API path even if the underlying texture is unchanged.
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("image_pipeline: no GPU adapter — skipping");
        return;
    };

    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);

    let red: Vec<u8> = (0..16).flat_map(|_| [255u8, 0, 0, 255]).collect();
    let uv_rect = upload_patch(&mut atlas, &queue, 4, 4, &red);

    let mut pipeline = ImagePipeline::new(&device, format, &bgl, &atlas);
    pipeline.rebuild_atlas_bg(&device, &atlas);

    let instances = vec![ImageInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        uv_rect,
        tint: [1.0; 4],
    }];

    let buf = run_image_test(
        w,
        h,
        &instances,
        &device,
        &queue,
        &mut pipeline,
        "image-rebuild-bg-pass",
    );
    drop(atlas);

    common::assert_pixel(&buf, w, w / 2, h / 2, [255, 0, 0, 255], 2);
}

#[test]
fn empty_instances_is_a_noop() {
    let Some((device, queue)) = common::make_headless_device() else {
        return;
    };
    let format = TextureFormat::Bgra8UnormSrgb;
    let bgl = viewport_bind_group_layout(&device);
    let atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);
    let mut pipeline = ImagePipeline::new(&device, format, &bgl, &atlas);
    let before = pipeline.capacity_bytes();
    pipeline.prepare(&device, &queue, &[]);
    assert_eq!(pipeline.capacity_bytes(), before);
}

#[test]
fn allocate_image_reserves_gutter_and_insets_uv() {
    // Mirror of `allocate_glyph_reserves_gutter_and_insets_uv`: the image path
    // must inset its uv_rect by exactly 1 texel per side so linear sampling at
    // a non-integer scale never bleeds from a packed neighbour.
    let Some((device, _queue)) = common::make_headless_device() else {
        eprintln!("image_pipeline: no GPU adapter — skipping");
        return;
    };
    let mut atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);

    // Nominal 16×16 image. Atlas reserves 18×18; uv_rect inset by 1/PAGE.
    let uv_a = allocate_image(&mut atlas, 16, 16).expect("allocate image A").uv_rect;
    let texel = 1.0 / PAGE_SIZE as f32;
    assert!(
        (uv_a[2] - uv_a[0] - 16.0 * texel).abs() < 1e-6,
        "inset uv width drift: got {} want {}",
        uv_a[2] - uv_a[0],
        16.0 * texel
    );
    assert!(
        (uv_a[3] - uv_a[1] - 16.0 * texel).abs() < 1e-6,
        "inset uv height drift: got {} want {}",
        uv_a[3] - uv_a[1],
        16.0 * texel
    );
    // Inset at least 1 texel inside the raw allocation's top-left.
    assert!(uv_a[0] >= texel - 1e-6);
    assert!(uv_a[1] >= texel - 1e-6);

    // Adjacent image on the same shelf: its inset uv must stay disjoint from A.
    let uv_b = allocate_image(&mut atlas, 16, 16).expect("allocate image B").uv_rect;
    let a_disjoint_b = uv_a[2] <= uv_b[0] + 1e-6
        || uv_b[2] <= uv_a[0] + 1e-6
        || uv_a[3] <= uv_b[1] + 1e-6
        || uv_b[3] <= uv_a[1] + 1e-6;
    assert!(a_disjoint_b, "image uv_rects overlap: A={uv_a:?} B={uv_b:?}");
}

#[test]
fn pad_rgba_with_gutter_zeros_border_and_copies_inner() {
    // 2×2 opaque-white image → 4×4 padded buffer: a transparent (zero) border
    // ring with the original 2×2 block centred. A bleed at the inset edge
    // therefore samples transparency, not a neighbour.
    let src: Vec<u8> = (0..4).flat_map(|_| [255u8, 255, 255, 255]).collect();
    let padded = pad_rgba_with_gutter(&src, 2, 2);
    assert_eq!(padded.len(), 4 * 4 * 4);

    let px = |x: usize, y: usize| {
        let off = (y * 4 + x) * 4;
        [padded[off], padded[off + 1], padded[off + 2], padded[off + 3]]
    };
    // Whole border ring is transparent.
    for i in 0..4 {
        assert_eq!(px(i, 0), [0, 0, 0, 0], "top row not zero at x={i}");
        assert_eq!(px(i, 3), [0, 0, 0, 0], "bottom row not zero at x={i}");
        assert_eq!(px(0, i), [0, 0, 0, 0], "left col not zero at y={i}");
        assert_eq!(px(3, i), [0, 0, 0, 0], "right col not zero at y={i}");
    }
    // Inner 2×2 is the original opaque white.
    for y in 1..3 {
        for x in 1..3 {
            assert_eq!(px(x, y), [255, 255, 255, 255], "inner ({x},{y}) not copied");
        }
    }
}
