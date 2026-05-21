//! Atlas-sampled image pipeline (Phase 4).
//!
//! Renders every [`ImageInstance`] from a [`crate::Scene`] in a single
//! `draw(0..6, 0..N)` call, sampling from the shared color atlas
//! ([`crate::atlas::Atlas`] in `Format::Rgba8UnormSrgb`).
//!
//! # Two-phase API (red-team P0-2)
//!
//! Same shape as [`crate::InstancedRectPipeline`]:
//!
//! 1. [`prepare`] — `&mut self`, called BEFORE `begin_render_pass`. Uploads
//!    instance data via `Queue::write_buffer`.
//! 2. [`record`] — `&self`, called inside the render pass. Sets pipeline +
//!    bind groups + vertex buffers, issues one `pass.draw`.
//!
//! # Atlas binding
//!
//! Pipeline does NOT own the atlas. `Renderer` (Phase 7) owns one shared
//! color atlas; the pipeline holds a `BindGroup` referencing the atlas's
//! texture view plus a linear sampler. When the atlas re-allocates the
//! underlying texture (page growth in a future phase), the renderer must
//! call [`rebuild_atlas_bg`] with the new `Atlas` so the bind group stays valid.
//!
//! # Color contract
//!
//! `ImageInstance.tint` is **linear, premultiplied** RGBA — `[1, 1, 1, 1]`
//! is the no-op identity. Atlas pixels are straight RGBA (e.g. `image::open()`
//! decode); the fragment shader premultiplies at sample time. Blend is
//! `One/OneMinusSrcAlpha` on both color and alpha.
//!
//! [`prepare`]: ImagePipeline::prepare
//! [`record`]: ImagePipeline::record
//! [`rebuild_atlas_bg`]: ImagePipeline::rebuild_atlas_bg

use std::mem;
use std::ops::Range;

use wgpu::{
    BindGroup, BindGroupLayout, BlendComponent, BlendFactor, BlendOperation, BlendState, Buffer,
    BufferAddress, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, Device,
    FragmentState, MultisampleState, PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology,
    Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor, Sampler, ShaderModuleDescriptor,
    ShaderSource, TextureFormat, VertexAttribute, VertexBufferLayout, VertexFormat, VertexState,
    VertexStepMode,
};

use crate::atlas::{Atlas, AtlasAllocation, AtlasError, Format, PAGE_SIZE};
use crate::pipeline_shared::{atlas_bind_group, atlas_bind_group_layout, atlas_linear_sampler};
use crate::scene::ImageInstance;

/// Initial instance-buffer capacity (matches `InstancedRectPipeline`).
const MIN_INSTANCES: u64 = 64;

/// Soft cap above which `prepare` warn-logs once per session.
const WARN_INSTANCE_CAP: usize = 1_000_000;

/// Per-instance attributes for `ImageInstance` (48 bytes: rect, uv_rect, tint).
/// Locations 1..=3 reserved for instance data; location 0 is the unit-quad
/// corner.
const INSTANCE_ATTRS: [VertexAttribute; 3] = [
    VertexAttribute {
        format: VertexFormat::Float32x4,
        offset: 0,
        shader_location: 1,
    },
    VertexAttribute {
        format: VertexFormat::Float32x4,
        offset: 16,
        shader_location: 2,
    },
    VertexAttribute {
        format: VertexFormat::Float32x4,
        offset: 32,
        shader_location: 3,
    },
];

const VERTEX_ATTRS: [VertexAttribute; 1] = [VertexAttribute {
    format: VertexFormat::Float32x2,
    offset: 0,
    shader_location: 0,
}];

/// GPU pipeline that samples the color atlas and renders all `ImageInstance`s
/// in a [`crate::Scene`] via instanced draw calls.
pub struct ImagePipeline {
    pipeline: RenderPipeline,
    instance_buffer: Buffer,
    instance_capacity_bytes: u64,
    last_instance_count: u32,
    /// Bind-group layout for the atlas (`@group(1)`). Held so we can rebuild
    /// the bind group when the atlas texture view changes.
    atlas_bgl: BindGroupLayout,
    atlas_bg: BindGroup,
    sampler: Sampler,
    warned_over_cap: bool,
}

impl ImagePipeline {
    /// Build the pipeline. `viewport_bgl` comes from
    /// [`crate::pipeline_shared::viewport_bind_group_layout`]; `image_atlas`
    /// is the renderer-owned color atlas (must be `Rgba8UnormSrgb`).
    pub fn new(
        device: &Device,
        surface_format: TextureFormat,
        viewport_bgl: &BindGroupLayout,
        image_atlas: &Atlas,
    ) -> Self {
        // Phase 1 contract: surface must be sRGB so hw handles linear→sRGB
        // encoding. Non-sRGB surfaces compile and run but produce washed-out
        // output (red-team P2-12).
        assert!(
            matches!(
                surface_format,
                TextureFormat::Bgra8UnormSrgb
                    | TextureFormat::Rgba8UnormSrgb
                    | TextureFormat::Bgra8Unorm
                    | TextureFormat::Rgba8Unorm
            ),
            "ImagePipeline expects sRGB or UNORM surface format; got {surface_format:?}",
        );
        // Mirror of GlyphPipeline's R8Unorm guard: catch the "glyph mask atlas
        // wired to image pipeline" footgun. The shared atlas BGL accepts
        // either format (filterable Float), so wgpu validation never sees the
        // mistake; output would silently broadcast `tex.r` to all channels.
        assert_eq!(
            image_atlas.format(),
            Format::Rgba8UnormSrgb,
            "ImagePipeline requires an Rgba8UnormSrgb atlas; got {:?}",
            image_atlas.format(),
        );
        let atlas_view = image_atlas.texture_view();
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("image.wgsl"),
            source: ShaderSource::Wgsl(include_str!("shaders/image.wgsl").into()),
        });

        let atlas_bgl = atlas_bind_group_layout(device, "slate-image-atlas-bgl");
        let sampler = atlas_linear_sampler(device, "slate-image-atlas-linear");
        let atlas_bg = atlas_bind_group(
            device,
            "slate-image-atlas-bg",
            &atlas_bgl,
            atlas_view,
            &sampler,
        );

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("image-pipeline-layout"),
            bind_group_layouts: &[Some(viewport_bgl), Some(&atlas_bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("image-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    VertexBufferLayout {
                        array_stride: mem::size_of::<[f32; 2]>() as BufferAddress,
                        step_mode: VertexStepMode::Vertex,
                        attributes: &VERTEX_ATTRS,
                    },
                    VertexBufferLayout {
                        array_stride: mem::size_of::<ImageInstance>() as BufferAddress,
                        step_mode: VertexStepMode::Instance,
                        attributes: &INSTANCE_ATTRS,
                    },
                ],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format: surface_format,
                    // Phase 1 premultiplied contract.
                    blend: Some(BlendState {
                        color: BlendComponent {
                            src_factor: BlendFactor::One,
                            dst_factor: BlendFactor::OneMinusSrcAlpha,
                            operation: BlendOperation::Add,
                        },
                        alpha: BlendComponent {
                            src_factor: BlendFactor::One,
                            dst_factor: BlendFactor::OneMinusSrcAlpha,
                            operation: BlendOperation::Add,
                        },
                    }),
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let instance_capacity_bytes = MIN_INSTANCES * mem::size_of::<ImageInstance>() as u64;
        let instance_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("image-instances"),
            size: instance_capacity_bytes,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            instance_buffer,
            instance_capacity_bytes,
            last_instance_count: 0,
            atlas_bgl,
            atlas_bg,
            sampler,
            warned_over_cap: false,
        }
    }

    /// Rebuild the atlas bind group from a new `Atlas`. Call this when the
    /// atlas re-allocates its underlying texture (Phase 7+ atlas growth).
    /// Panics if `image_atlas` has the wrong format (must be `Rgba8UnormSrgb`).
    pub fn rebuild_atlas_bg(&mut self, device: &Device, image_atlas: &Atlas) {
        assert_eq!(
            image_atlas.format(),
            Format::Rgba8UnormSrgb,
            "ImagePipeline::rebuild_atlas_bg requires an Rgba8UnormSrgb atlas; got {:?}",
            image_atlas.format(),
        );
        let atlas_view = image_atlas.texture_view();
        self.atlas_bg = atlas_bind_group(
            device,
            "slate-image-atlas-bg",
            &self.atlas_bgl,
            atlas_view,
            &self.sampler,
        );
    }

    /// Upload `instances` to the per-instance buffer. Call once per frame
    /// **before** `begin_render_pass`.
    pub fn prepare(&mut self, device: &Device, queue: &Queue, instances: &[ImageInstance]) {
        if instances.is_empty() {
            self.last_instance_count = 0;
            return;
        }
        if instances.len() > WARN_INSTANCE_CAP && !self.warned_over_cap {
            log::warn!(
                "ImagePipeline: {} instances exceeds soft cap {}",
                instances.len(),
                WARN_INSTANCE_CAP,
            );
            self.warned_over_cap = true;
        }
        self.ensure_capacity(device, instances.len());
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));
        self.last_instance_count = instances.len() as u32;
    }

    /// Record one instanced draw covering `range` of the buffer uploaded in
    /// the matching `prepare` call.
    pub fn record<'a>(
        &'a self,
        pass: &mut RenderPass<'a>,
        viewport_bg: &'a BindGroup,
        unit_quad: &'a Buffer,
        range: Range<u32>,
    ) {
        if range.is_empty() {
            return;
        }
        debug_assert!(
            range.end <= self.last_instance_count,
            "ImagePipeline::record: range {:?} extends past last_instance_count={}; \
             prepare() was called with too few instances or not at all",
            range,
            self.last_instance_count,
        );
        // Clamp so release builds don't silently draw stale instances past
        // last_instance_count. The debug_assert above still catches caller
        // bugs in debug builds.
        let end = range.end.min(self.last_instance_count);
        if end <= range.start {
            return;
        }
        let stride = mem::size_of::<ImageInstance>() as BufferAddress;
        let byte_range = (range.start as BufferAddress * stride)..(end as BufferAddress * stride);

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, viewport_bg, &[]);
        pass.set_bind_group(1, &self.atlas_bg, &[]);
        pass.set_vertex_buffer(0, unit_quad.slice(..));
        pass.set_vertex_buffer(1, self.instance_buffer.slice(byte_range));
        pass.draw(0..6, 0..(end - range.start));
    }

    /// Current capacity in bytes — exposed for tests / observability.
    pub fn capacity_bytes(&self) -> u64 {
        self.instance_capacity_bytes
    }

    fn ensure_capacity(&mut self, device: &Device, n: usize) {
        let stride = mem::size_of::<ImageInstance>() as u64;
        let needed = (n as u64)
            .checked_mul(stride)
            .expect("instance count × stride overflows u64 (impossibly large frame)");
        if needed <= self.instance_capacity_bytes {
            return;
        }
        let min = MIN_INSTANCES * stride;
        let new_cap = needed.next_power_of_two().max(min);
        self.instance_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("image-instances"),
            size: new_cap,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity_bytes = new_cap;
    }
}

/// Allocate an image slot with a 1-texel transparent gutter on every side.
///
/// Mirrors [`crate::allocate_glyph`]: the atlas reserves a
/// `(width + 2) × (height + 2)` region but the returned `uv_rect` is inset by
/// exactly 1 texel per side, so linear sampling at a non-integer scale never
/// pulls texels from a neighbouring image. Caller uploads the padded buffer
/// from [`pad_rgba_with_gutter`] (the gutter is zero-filled → samples read
/// transparent). Returns the [`AtlasAllocation`] with its `uv_rect` already
/// inset (and the atlas's `alloc_id` + liveness `token` carried through).
pub fn allocate_image(
    atlas: &mut Atlas,
    width: u32,
    height: u32,
) -> Result<AtlasAllocation, AtlasError> {
    debug_assert!(
        width > 0 && height > 0,
        "image dimensions must be non-zero (got {width}×{height})",
    );
    let padded_w = width.checked_add(2).ok_or(AtlasError::TooLarge {
        requested: (width, height),
        max: PAGE_SIZE,
    })?;
    let padded_h = height.checked_add(2).ok_or(AtlasError::TooLarge {
        requested: (width, height),
        max: PAGE_SIZE,
    })?;
    let alloc = atlas.allocate(padded_w, padded_h)?;
    // 1-texel inset in normalized coords; query the atlas so multi-page atlases
    // with differing page sizes stay correct.
    let texel = atlas.texel_size_uv();
    Ok(AtlasAllocation {
        uv_rect: [
            alloc.uv_rect[0] + texel,
            alloc.uv_rect[1] + texel,
            alloc.uv_rect[2] - texel,
            alloc.uv_rect[3] - texel,
        ],
        ..alloc
    })
}

/// Pad a `w × h` straight-RGBA image with a 1-pixel zero-fill (transparent)
/// gutter on all sides, producing the `(w+2) × (h+2)` buffer that the
/// [`allocate_image`] slot expects.
pub fn pad_rgba_with_gutter(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    debug_assert!(
        src.len() == w as usize * h as usize * 4,
        "src must be tight RGBA8 of w×h ({w}×{h})"
    );
    let pw = (w + 2) as usize;
    let ph = (h + 2) as usize;
    let row = w as usize * 4;
    let mut buf = vec![0u8; pw * ph * 4];
    for y in 0..h as usize {
        let src_off = y * row;
        let dst_off = ((y + 1) * pw + 1) * 4;
        buf[dst_off..dst_off + row].copy_from_slice(&src[src_off..src_off + row]);
    }
    buf
}
