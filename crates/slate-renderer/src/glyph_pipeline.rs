//! Atlas-sampled glyph pipeline.
//!
//! Renders every [`GlyphInstance`] from a [`crate::Scene`] in a single
//! `draw(0..6, 0..N)` call, sampling from the shared R8 alpha atlas
//! ([`crate::atlas::Atlas`] in `Format::R8Unorm`) tinted by per-instance
//! premultiplied ink color.
//!
//! # Two-phase API
//!
//! Same shape as [`crate::ImagePipeline`] / [`crate::InstancedRectPipeline`]:
//!
//! 1. [`prepare`] — `&mut self`, called BEFORE `begin_render_pass`. Uploads
//!    instance data via `Queue::write_buffer`.
//! 2. [`record`] — `&self`, called inside the render pass. Sets pipeline +
//!    bind groups + vertex buffers, issues one `pass.draw`.
//!
//! # Atlas binding
//!
//! Pipeline does NOT own the atlas. `Renderer` owns one shared glyph atlas;
//! the pipeline holds a `BindGroup` referencing the atlas's texture view plus
//! a linear sampler. When the atlas re-allocates the underlying texture, the
//! renderer must call [`rebuild_atlas_bg`] with the new `Atlas` so the bind
//! group stays valid.
//!
//! # Color contract
//!
//! `GlyphInstance.color` is **linear, premultiplied** RGBA — the fragment
//! shader scales it by the sampled alpha mask, keeping the output
//! premultiplied. Atlas pixels are 8-bit linear alpha (R8Unorm; alpha is
//! never gamma-encoded). Blend is `One/OneMinusSrcAlpha` on both color and
//! alpha.
//!
//! # 1px gutter
//!
//! Glyph producers and the demo seeder MUST go through [`allocate_glyph`]
//! (not `Atlas::allocate` directly): it inflates the request by 1 transparent
//! texel on every side and returns a uv_rect inset by 1 texel. This stops
//! the linear sampler from bleeding between adjacent glyphs at fractional
//! sub-pixel offsets.
//!
//! [`prepare`]: GlyphPipeline::prepare
//! [`record`]: GlyphPipeline::record
//! [`rebuild_atlas_bg`]: GlyphPipeline::rebuild_atlas_bg

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
use crate::scene::GlyphInstance;

/// Initial instance-buffer capacity (matches sibling pipelines).
const MIN_INSTANCES: u64 = 64;

/// Soft cap above which `prepare` warn-logs once per session.
const WARN_INSTANCE_CAP: usize = 1_000_000;

/// Per-instance attributes for [`GlyphInstance`] (64 bytes: rect, uv_rect,
/// color, sub_pixel_variant + pad). Locations 1..=4 reserved for instance
/// data; location 0 is the unit-quad corner.
const INSTANCE_ATTRS: [VertexAttribute; 4] = [
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
    VertexAttribute {
        format: VertexFormat::Uint32,
        offset: 48,
        shader_location: 4,
    },
];

const VERTEX_ATTRS: [VertexAttribute; 1] = [VertexAttribute {
    format: VertexFormat::Float32x2,
    offset: 0,
    shader_location: 0,
}];

/// GPU pipeline that samples the R8 glyph atlas and renders all
/// `GlyphInstance`s in a [`crate::Scene`] via instanced draw calls.
pub struct GlyphPipeline {
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

impl GlyphPipeline {
    /// Build the pipeline. `viewport_bgl` comes from
    /// [`crate::pipeline_shared::viewport_bind_group_layout`]; `glyph_atlas`
    /// is the renderer-owned alpha-mask atlas (must be `R8Unorm`).
    pub fn new(
        device: &Device,
        surface_format: TextureFormat,
        viewport_bgl: &BindGroupLayout,
        glyph_atlas: &Atlas,
    ) -> Self {
        // Surface must be sRGB so hw handles linear→sRGB encoding.
        // Glyph atlas itself is linear R8 — separate concern.
        assert!(
            matches!(
                surface_format,
                TextureFormat::Bgra8UnormSrgb
                    | TextureFormat::Rgba8UnormSrgb
                    | TextureFormat::Bgra8Unorm
                    | TextureFormat::Rgba8Unorm
            ),
            "GlyphPipeline expects sRGB or UNORM surface format; got {surface_format:?}",
        );
        // Red-team H2: catch the "image atlas wired to glyph pipeline" footgun
        // — the bind-group layout's `Float { filterable: true }` accepts both
        // R8Unorm and Rgba8UnormSrgb, so wgpu validation never sees the
        // mistake; output would silently be `texture.r` of an sRGB color
        // texel used as alpha mask.
        assert_eq!(
            glyph_atlas.format(),
            Format::R8Unorm,
            "GlyphPipeline requires an R8Unorm atlas; got {:?}",
            glyph_atlas.format(),
        );
        let atlas_view = glyph_atlas.texture_view();
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("glyph.wgsl"),
            source: ShaderSource::Wgsl(include_str!("shaders/glyph.wgsl").into()),
        });

        let atlas_bgl = atlas_bind_group_layout(device, "slate-glyph-atlas-bgl");
        let sampler = atlas_linear_sampler(device, "slate-glyph-atlas-linear");
        let atlas_bg = atlas_bind_group(
            device,
            "slate-glyph-atlas-bg",
            &atlas_bgl,
            atlas_view,
            &sampler,
        );

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("glyph-pipeline-layout"),
            bind_group_layouts: &[Some(viewport_bgl), Some(&atlas_bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("glyph-pipeline"),
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
                        array_stride: mem::size_of::<GlyphInstance>() as BufferAddress,
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
                    // Premultiplied-alpha blend: One/OneMinusSrcAlpha on both color and alpha.
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

        let instance_capacity_bytes = MIN_INSTANCES * mem::size_of::<GlyphInstance>() as u64;
        let instance_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("glyph-instances"),
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
    /// atlas re-allocates its underlying texture (e.g. on atlas growth).
    /// Panics if `glyph_atlas` has the wrong format (must be `R8Unorm`).
    pub fn rebuild_atlas_bg(&mut self, device: &Device, glyph_atlas: &Atlas) {
        assert_eq!(
            glyph_atlas.format(),
            Format::R8Unorm,
            "GlyphPipeline::rebuild_atlas_bg requires an R8Unorm atlas; got {:?}",
            glyph_atlas.format(),
        );
        let atlas_view = glyph_atlas.texture_view();
        self.atlas_bg = atlas_bind_group(
            device,
            "slate-glyph-atlas-bg",
            &self.atlas_bgl,
            atlas_view,
            &self.sampler,
        );
    }

    /// Upload `instances` to the per-instance buffer. Call once per frame
    /// **before** `begin_render_pass`.
    pub fn prepare(&mut self, device: &Device, queue: &Queue, instances: &[GlyphInstance]) {
        if instances.is_empty() {
            self.last_instance_count = 0;
            return;
        }
        if instances.len() > WARN_INSTANCE_CAP && !self.warned_over_cap {
            log::warn!(
                "GlyphPipeline: {} instances exceeds soft cap {}",
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
            "GlyphPipeline::record: range {:?} extends past last_instance_count={}; \
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
        let stride = mem::size_of::<GlyphInstance>() as BufferAddress;
        let byte_range = (range.start as BufferAddress * stride)..(end as BufferAddress * stride);

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, viewport_bg, &[]);
        pass.set_bind_group(1, &self.atlas_bg, &[]);
        pass.set_vertex_buffer(0, unit_quad.slice(..));
        pass.set_vertex_buffer(1, self.instance_buffer.slice(byte_range));
        pass.draw(0..6, 0..(end - range.start));

        #[cfg(feature = "profiling")]
        crate::profiling::PAINT_CMD_COUNT
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Current capacity in bytes — exposed for tests / observability.
    pub fn capacity_bytes(&self) -> u64 {
        self.instance_capacity_bytes
    }

    fn ensure_capacity(&mut self, device: &Device, n: usize) {
        let stride = mem::size_of::<GlyphInstance>() as u64;
        let needed = (n as u64)
            .checked_mul(stride)
            .expect("instance count × stride overflows u64 (impossibly large frame)");
        if needed <= self.instance_capacity_bytes {
            return;
        }
        let min = MIN_INSTANCES * stride;
        let new_cap = needed.next_power_of_two().max(min);
        self.instance_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("glyph-instances"),
            size: new_cap,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity_bytes = new_cap;
    }
}

/// Allocate a glyph slot with a 1-texel transparent gutter on every side.
///
/// The atlas reserves a `(width + 2) × (height + 2)` region but the returned
/// `uv_rect` is inset by exactly 1 texel in each dimension, so linear
/// sampling at a sub-pixel offset never pulls texels from a neighbouring
/// glyph.
///
/// Caller is responsible for uploading the gutter pixels (zero-fill is fine
/// — the pipeline never samples outside `uv_rect`). Returns the
/// [`AtlasAllocation`] with its `uv_rect` already inset (and the atlas's
/// `alloc_id` + liveness `token` carried through).
pub fn allocate_glyph(
    atlas: &mut Atlas,
    width: u32,
    height: u32,
) -> Result<AtlasAllocation, AtlasError> {
    // Zero-sized glyph would yield a 2×2 alloc with a fully-collapsed inset
    // uv_rect (sampler reads only gutter texels). Catch in debug; producers
    // are expected to filter zero-metric glyphs upstream.
    debug_assert!(
        width > 0 && height > 0,
        "glyph dimensions must be non-zero (got {width}×{height})",
    );
    // u32::MAX would panic on the +2 add; surface as TooLarge instead.
    let padded_w = width.checked_add(2).ok_or(AtlasError::TooLarge {
        requested: (width, height),
        max: PAGE_SIZE,
    })?;
    let padded_h = height.checked_add(2).ok_or(AtlasError::TooLarge {
        requested: (width, height),
        max: PAGE_SIZE,
    })?;
    let alloc = atlas.allocate(padded_w, padded_h)?;
    // 1-texel inset in normalized coords. Query the atlas so future multi-page
    // atlases with differing page sizes stay correct.
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
