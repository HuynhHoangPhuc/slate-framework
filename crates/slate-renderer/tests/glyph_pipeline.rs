//! Headless integration tests for [`slate_renderer::GlyphPipeline`] (Phase 5).
//!
//! Covers (per phase-05 step 5):
//!   - Mask render: 16×16 fully-opaque alpha patch tinted premultiplied red
//!     → red center pixel.
//!   - Premultiplied tint × half-alpha mask: 16×16 alpha=128 patch + premul
//!     red ink → sRGB-encoded `(188, 0, 0, 128)` ±2/255.
//!   - `allocate_glyph` 1px gutter: helper reserves (w+2)×(h+2) and returns
//!     a uv_rect inset by exactly 1/PAGE_SIZE on each side.
//!   - Capacity grows monotonically; empty `prepare` is a no-op.

mod common;

use slate_renderer::atlas::{Atlas, Format, PAGE_SIZE};
use slate_renderer::{
    GlyphInstance, GlyphPipeline, Lpx, ViewportUniform, allocate_glyph, create_unit_quad,
    linear_to_srgb_channel, viewport_bind_group_layout,
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

/// Upload a `width × height` R8 mask via [`allocate_glyph`] (so the 1-texel
/// gutter is included). Returns the inset uv_rect.
fn upload_mask(
    atlas: &mut Atlas,
    queue: &Queue,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> [f32; 4] {
    let alloc = allocate_glyph(atlas, width, height).expect("allocate_glyph failed");
    // The atlas allocation is (width+2)×(height+2); upload zeros for the
    // gutter and the caller-supplied mask in the inner 1..=width region.
    let aw = width + 2;
    let ah = height + 2;
    let mut padded = vec![0u8; (aw * ah) as usize];
    for y in 0..height {
        let src_off = (y * width) as usize;
        let dst_off = ((y + 1) * aw + 1) as usize;
        padded[dst_off..dst_off + width as usize]
            .copy_from_slice(&pixels[src_off..src_off + width as usize]);
    }
    atlas.upload(queue, alloc.alloc_id, &padded);
    alloc.uv_rect
}

fn run_glyph_test(
    w: u32,
    h: u32,
    instances: &[GlyphInstance],
    device: &Device,
    queue: &Queue,
    pipeline: &mut GlyphPipeline,
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
fn opaque_mask_renders_premul_red() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("glyph_pipeline: no GPU adapter — skipping");
        return;
    };
    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::R8Unorm);

    // 16×16 fully-opaque mask.
    let mask = vec![255u8; 16 * 16];
    let uv_rect = upload_mask(&mut atlas, &queue, 16, 16, &mask);

    let mut pipeline = GlyphPipeline::new(&device, format, &bgl, &atlas);

    let instances = vec![GlyphInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        uv_rect,
        // Premultiplied red, full alpha.
        color: [1.0, 0.0, 0.0, 1.0],
        sub_pixel_variant: 0,
        _pad: [0; 3],
    }];

    let buf = run_glyph_test(
        w,
        h,
        &instances,
        &device,
        &queue,
        &mut pipeline,
        "glyph-opaque-pass",
    );
    drop(atlas);

    common::assert_pixel(&buf, w, w / 2, h / 2, [255, 0, 0, 255], 2);
}

#[test]
fn half_alpha_mask_with_red_ink_premultiplies() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("glyph_pipeline: no GPU adapter — skipping");
        return;
    };
    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::R8Unorm);

    // 16×16 mask at alpha=128 (≈0.502 linear).
    let mask = vec![128u8; 16 * 16];
    let uv_rect = upload_mask(&mut atlas, &queue, 16, 16, &mask);

    let mut pipeline = GlyphPipeline::new(&device, format, &bgl, &atlas);

    // Premultiplied red, full ink alpha. Output = color * mask =
    // (0.502, 0, 0, 0.502). Surface sRGB encodes RGB; alpha is linear.
    let instances = vec![GlyphInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        uv_rect,
        color: [1.0, 0.0, 0.0, 1.0],
        sub_pixel_variant: 0,
        _pad: [0; 3],
    }];

    let buf = run_glyph_test(
        w,
        h,
        &instances,
        &device,
        &queue,
        &mut pipeline,
        "glyph-half-alpha-pass",
    );
    drop(atlas);

    let mask_linear = 128_f32 / 255.0;
    let r_premul_linear = 1.0 * mask_linear;
    let r_srgb = (linear_to_srgb_channel(r_premul_linear) * 255.0).round() as u8;
    let a_byte = (mask_linear * 255.0).round() as u8;
    common::assert_pixel(&buf, w, w / 2, h / 2, [r_srgb, 0, 0, a_byte], 2);
}

#[test]
fn allocate_glyph_reserves_gutter_and_insets_uv() {
    let Some((device, _queue)) = common::make_headless_device() else {
        eprintln!("glyph_pipeline: no GPU adapter — skipping");
        return;
    };
    let mut atlas = Atlas::new(&device, Format::R8Unorm);

    // First glyph: nominal 16×16. Atlas reserves 18×18; uv_rect inset by 1/PAGE.
    let uv_a = allocate_glyph(&mut atlas, 16, 16).expect("allocate glyph A").uv_rect;
    let texel = 1.0 / PAGE_SIZE as f32;
    // Width & height of the inset uv_rect should equal 16 texels exactly.
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
    // Inset must be at least 1 texel from the page edge (allocate_glyph
    // shifted it inward by 1 texel from the raw allocation).
    assert!(uv_a[0] >= texel - 1e-6);
    assert!(uv_a[1] >= texel - 1e-6);

    // Second glyph: lands adjacent on the same shelf. Its uv must not overlap
    // glyph A's uv_rect (gutter ensures separation in atlas, inset preserves
    // separation in normalized uv).
    let uv_b = allocate_glyph(&mut atlas, 16, 16).expect("allocate glyph B").uv_rect;
    let a_disjoint_b = uv_a[2] <= uv_b[0] + 1e-6
        || uv_b[2] <= uv_a[0] + 1e-6
        || uv_a[3] <= uv_b[1] + 1e-6
        || uv_b[3] <= uv_a[1] + 1e-6;
    assert!(
        a_disjoint_b,
        "glyph uv_rects overlap: A={:?} B={:?}",
        uv_a, uv_b
    );
}

#[test]
fn capacity_grows_monotonically() {
    let Some((device, queue)) = common::make_headless_device() else {
        return;
    };
    let format = TextureFormat::Bgra8UnormSrgb;
    let bgl = viewport_bind_group_layout(&device);
    let atlas = Atlas::new(&device, Format::R8Unorm);
    let mut pipeline = GlyphPipeline::new(&device, format, &bgl, &atlas);

    let stride = std::mem::size_of::<GlyphInstance>() as u64;
    let initial = pipeline.capacity_bytes();
    assert!(initial >= 64 * stride);

    let dummy = GlyphInstance {
        rect: [Lpx(0.0); 4],
        uv_rect: [0.0, 0.0, 1.0, 1.0],
        color: [1.0; 4],
        sub_pixel_variant: 0,
        _pad: [0; 3],
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
fn empty_instances_is_a_noop() {
    let Some((device, queue)) = common::make_headless_device() else {
        return;
    };
    let format = TextureFormat::Bgra8UnormSrgb;
    let bgl = viewport_bind_group_layout(&device);
    let atlas = Atlas::new(&device, Format::R8Unorm);
    let mut pipeline = GlyphPipeline::new(&device, format, &bgl, &atlas);
    let before = pipeline.capacity_bytes();
    pipeline.prepare(&device, &queue, &[]);
    assert_eq!(pipeline.capacity_bytes(), before);
}

#[test]
fn rebuild_atlas_bg_does_not_break_subsequent_draws() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("glyph_pipeline: no GPU adapter — skipping");
        return;
    };
    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::R8Unorm);

    let mask = vec![255u8; 16 * 16];
    let uv_rect = upload_mask(&mut atlas, &queue, 16, 16, &mask);

    let mut pipeline = GlyphPipeline::new(&device, format, &bgl, &atlas);
    pipeline.rebuild_atlas_bg(&device, &atlas);

    let instances = vec![GlyphInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        uv_rect,
        color: [1.0, 1.0, 1.0, 1.0],
        sub_pixel_variant: 0,
        _pad: [0; 3],
    }];

    let buf = run_glyph_test(
        w,
        h,
        &instances,
        &device,
        &queue,
        &mut pipeline,
        "glyph-rebuild-bg-pass",
    );
    drop(atlas);

    common::assert_pixel(&buf, w, w / 2, h / 2, [255, 255, 255, 255], 2);
}

#[test]
fn gutter_blocks_neighbor_bleed_at_render_time() {
    // Red-team H1: the geometry-only gutter test does not actually prove
    // linear filtering can't reach a hot neighbor. Allocate two adjacent
    // 16×16 glyphs via `allocate_glyph` (which inserts the gutter), upload
    // glyph A as fully transparent (mask=0) and glyph B as fully opaque
    // (mask=255) into adjacent atlas slots. Render glyph A scaled up to
    // 32×32 — every output pixel must be transparent. If the inset shrinks
    // (e.g. someone sets `texel = 0.0`) the linear sampler at the right
    // edge of A would partially read B's 255 mask and the rendered alpha
    // jumps above 0.
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("glyph_pipeline: no GPU adapter — skipping");
        return;
    };
    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::R8Unorm);

    let zeros = vec![0u8; 16 * 16];
    let ones = vec![255u8; 16 * 16];
    let uv_a = upload_mask(&mut atlas, &queue, 16, 16, &zeros);
    let _uv_b = upload_mask(&mut atlas, &queue, 16, 16, &ones);

    let mut pipeline = GlyphPipeline::new(&device, format, &bgl, &atlas);

    // Render only glyph A. Premultiplied red ink, full ink alpha.
    let instances = vec![GlyphInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        uv_rect: uv_a,
        color: [1.0, 0.0, 0.0, 1.0],
        sub_pixel_variant: 0,
        _pad: [0; 3],
    }];

    let buf = run_glyph_test(
        w,
        h,
        &instances,
        &device,
        &queue,
        &mut pipeline,
        "glyph-gutter-bleed-pass",
    );

    // Sample interior + edges: every pixel inside the rendered quad must
    // come from the all-zero mask of glyph A. Edge pixels are the bleed
    // candidates (linear filter spread at uv boundaries).
    for &(x, y) in &[
        (0u32, 0u32),
        (w - 1, 0),
        (0, h - 1),
        (w - 1, h - 1),
        (w / 2, h / 2),
        (w - 1, h / 2),
    ] {
        common::assert_pixel(&buf, w, x, y, [0, 0, 0, 0], 2);
    }
}

#[test]
fn ten_glyphs_in_one_draw_render_correct_centers() {
    // Red-team M1 / phase-05 success criterion: prove `draw(0..6, 0..N)`
    // for N>1 actually walks per-instance attributes correctly. A bug in
    // INSTANCE_ATTRS stride or a misread of `Uint32` at offset 48 only
    // surfaces when the GPU fetches instance ≥1.
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("glyph_pipeline: no GPU adapter — skipping");
        return;
    };
    let format = TextureFormat::Bgra8UnormSrgb;
    let n: u32 = 10;
    let cell = 16u32;
    let (w, h) = (cell * n, cell);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::R8Unorm);

    // Single shared opaque mask — geometry is what's under test, not mask
    // content.
    let mask = vec![255u8; (cell * cell) as usize];
    let uv_rect = upload_mask(&mut atlas, &queue, cell, cell, &mask);

    let mut pipeline = GlyphPipeline::new(&device, format, &bgl, &atlas);

    // Alternating red / green premultiplied ink — distinct per-instance
    // value so a stride bug shows up as the wrong color landing in a cell.
    let instances: Vec<GlyphInstance> = (0..n)
        .map(|i| {
            let color = if i % 2 == 0 {
                [1.0, 0.0, 0.0, 1.0]
            } else {
                [0.0, 1.0, 0.0, 1.0]
            };
            GlyphInstance {
                rect: [
                    Lpx((i * cell) as f32),
                    Lpx(0.0),
                    Lpx(cell as f32),
                    Lpx(cell as f32),
                ],
                uv_rect,
                color,
                sub_pixel_variant: i,
                _pad: [0; 3],
            }
        })
        .collect();

    let buf = run_glyph_test(
        w,
        h,
        &instances,
        &device,
        &queue,
        &mut pipeline,
        "glyph-multi-instance-pass",
    );

    // Sample center of every cell.
    for i in 0..n {
        let cx = i * cell + cell / 2;
        let cy = cell / 2;
        let expected = if i % 2 == 0 {
            [255, 0, 0, 255]
        } else {
            [0, 255, 0, 255]
        };
        common::assert_pixel(&buf, w, cx, cy, expected, 2);
    }
}

#[test]
fn sub_pixel_variant_does_not_affect_phase1_output() {
    // Direct proof that `sub_pixel_variant` (location 4) is read end-to-end:
    // render the same glyph twice with variant=0 vs variant=3 and assert byte-
    // identical output. In Phase 1 the fragment shader's variant term is
    // anchored as `+ vec4(0.0) * f32(variant)` so naga can't strip the
    // location read across DX12/Metal/Vulkan/SPIR-V. If a future refactor
    // dropped the anchor, naga would prune the attribute, the pipeline would
    // fail validation, AND this test would catch a behavioral divergence
    // *before* Phase 2's subpixel-AA path turns variant into a real input.
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("glyph_pipeline: no GPU adapter — skipping");
        return;
    };
    let format = TextureFormat::Bgra8UnormSrgb;
    let (w, h) = (32u32, 32u32);
    let bgl = viewport_bind_group_layout(&device);
    let mut atlas = Atlas::new(&device, Format::R8Unorm);

    let mask = vec![255u8; 16 * 16];
    let uv_rect = upload_mask(&mut atlas, &queue, 16, 16, &mask);

    let mut pipeline = GlyphPipeline::new(&device, format, &bgl, &atlas);

    let make_instance = |variant: u32| GlyphInstance {
        rect: [Lpx(0.0), Lpx(0.0), Lpx(w as f32), Lpx(h as f32)],
        uv_rect,
        color: [1.0, 0.0, 0.0, 1.0],
        sub_pixel_variant: variant,
        _pad: [0; 3],
    };

    let buf_v0 = run_glyph_test(
        w,
        h,
        &[make_instance(0)],
        &device,
        &queue,
        &mut pipeline,
        "glyph-variant-0-pass",
    );
    let buf_v3 = run_glyph_test(
        w,
        h,
        &[make_instance(3)],
        &device,
        &queue,
        &mut pipeline,
        "glyph-variant-3-pass",
    );

    assert_eq!(
        buf_v0, buf_v3,
        "Phase 1 output must be invariant to sub_pixel_variant; if this fires \
         either the location-4 anchor was removed or Phase 2 semantics leaked \
         into the shader",
    );
}
