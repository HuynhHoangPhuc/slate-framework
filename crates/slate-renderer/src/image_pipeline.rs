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
//! `TextureView` plus a linear sampler. When the atlas re-allocates the
//! underlying texture (page growth in a future phase), the renderer must
//! call [`rebuild_atlas_bg`] so the bind group's `TextureView` stays valid.
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
    AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, BlendComponent,
    BlendFactor, BlendOperation, BlendState, Buffer, BufferAddress, BufferDescriptor, BufferUsages,
    ColorTargetState, ColorWrites, Device, FilterMode, FragmentState, MipmapFilterMode,
    MultisampleState, PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology, Queue,
    RenderPass, RenderPipeline, RenderPipelineDescriptor, Sampler, SamplerBindingType,
    SamplerDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages, TextureFormat,
    TextureSampleType, TextureView, TextureViewDimension, VertexAttribute, VertexBufferLayout,
    VertexFormat, VertexState, VertexStepMode,
};

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
    /// [`crate::pipeline_shared::viewport_bind_group_layout`]; `atlas_view`
    /// is the current `TextureView` from the renderer-owned color atlas.
    pub fn new(
        device: &Device,
        surface_format: TextureFormat,
        viewport_bgl: &BindGroupLayout,
        atlas_view: &TextureView,
    ) -> Self {
        // Phase 1 contract: surface must be sRGB so hw handles linear→sRGB
        // encoding. Non-sRGB surfaces compile and run but produce washed-out
        // output (red-team P2-12).
        debug_assert!(
            matches!(
                surface_format,
                TextureFormat::Bgra8UnormSrgb | TextureFormat::Rgba8UnormSrgb
            ),
            "ImagePipeline expects an sRGB surface format (Bgra8UnormSrgb / \
             Rgba8UnormSrgb); got {surface_format:?}",
        );
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("image.wgsl"),
            source: ShaderSource::Wgsl(include_str!("shaders/image.wgsl").into()),
        });

        let atlas_bgl = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("slate-image-atlas-bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("slate-atlas-linear"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            // No mips in Phase 1.
            mipmap_filter: MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let atlas_bg = make_atlas_bind_group(device, &atlas_bgl, atlas_view, &sampler);

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

    /// Rebuild the atlas bind group with a new `TextureView`. Call this when
    /// the atlas re-allocates its underlying texture (Phase 7+ atlas growth).
    pub fn rebuild_atlas_bg(&mut self, device: &Device, atlas_view: &TextureView) {
        self.atlas_bg = make_atlas_bind_group(device, &self.atlas_bgl, atlas_view, &self.sampler);
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
        let stride = mem::size_of::<ImageInstance>() as BufferAddress;
        let byte_range =
            (range.start as BufferAddress * stride)..(range.end as BufferAddress * stride);

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, viewport_bg, &[]);
        pass.set_bind_group(1, &self.atlas_bg, &[]);
        pass.set_vertex_buffer(0, unit_quad.slice(..));
        pass.set_vertex_buffer(1, self.instance_buffer.slice(byte_range));
        pass.draw(0..6, 0..(range.end - range.start));
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

fn make_atlas_bind_group(
    device: &Device,
    layout: &BindGroupLayout,
    view: &TextureView,
    sampler: &Sampler,
) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label: Some("slate-image-atlas-bg"),
        layout,
        entries: &[
            BindGroupEntry {
                binding: 0,
                resource: BindingResource::TextureView(view),
            },
            BindGroupEntry {
                binding: 1,
                resource: BindingResource::Sampler(sampler),
            },
        ],
    })
}
