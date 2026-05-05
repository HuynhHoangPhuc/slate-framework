//! Instanced rounded-rect pipeline (Phase 2).
//!
//! Replaces Phase 0 / Phase 1's single-rect [`crate::RectPipeline`] (one
//! 48-byte uniform per draw) with an instanced pipeline that draws every
//! `RectInstance` from a [`crate::Scene`] in a single `draw(0..6, 0..N)` call.
//! Shader math is byte-for-byte identical — only the data path changes.
//!
//! # Two-phase API (red-team P0-2 fix)
//!
//! Splits work to avoid borrow-checker / drop-during-pass hazards:
//!
//! 1. [`prepare`] — mutates pipeline state (`ensure_capacity`, `write_buffer`)
//!    BEFORE `RenderPass` is started.
//! 2. [`record`] — only reads pipeline state and adds draw commands inside
//!    `begin_render_pass`. Borrow lifetimes: every `&'a` reference must
//!    outlive the pass.
//!
//! # Color contract
//!
//! `RectInstance.color` is **linear, premultiplied** RGBA — see
//! [`crate::srgb_u8_to_linear_premul`]. Blend state is `One/OneMinusSrcAlpha`
//! on both color and alpha, matching the Phase 1 reconciliation.
//!
//! [`prepare`]: InstancedRectPipeline::prepare
//! [`record`]: InstancedRectPipeline::record

use std::mem;
use std::ops::Range;

use wgpu::{
    BindGroup, BindGroupLayout, BlendComponent, BlendFactor, BlendOperation, BlendState, Buffer,
    BufferAddress, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, Device,
    FragmentState, MultisampleState, PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology,
    Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor, ShaderModuleDescriptor,
    ShaderSource, TextureFormat, VertexAttribute, VertexBufferLayout, VertexFormat, VertexState,
    VertexStepMode,
};

use crate::scene::RectInstance;

/// Initial instance-buffer capacity (kept tiny; doubles on overflow).
const MIN_INSTANCES: u64 = 64;

/// Soft cap above which `prepare` warn-logs once per session. 1 M instances ×
/// 48 B = 48 MB — far past any realistic frame.
const WARN_INSTANCE_CAP: usize = 1_000_000;

/// Vertex attributes for the per-instance buffer (`RectInstance`).
///
/// Offsets are computed from the field layout and asserted at compile time
/// against `size_of::<RectInstance>() == 48` (in `scene.rs`). Locations 1..=3
/// are reserved for instance data; location 0 is the per-vertex unit-quad
/// corner from [`crate::pipeline_shared::create_unit_quad`].
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
        format: VertexFormat::Float32,
        offset: 32,
        shader_location: 3,
    },
];

/// Vertex attribute for the per-vertex unit-quad buffer.
const VERTEX_ATTRS: [VertexAttribute; 1] = [VertexAttribute {
    format: VertexFormat::Float32x2,
    offset: 0,
    shader_location: 0,
}];

/// GPU pipeline that renders all `RectInstance`s in a [`crate::Scene`] via
/// instanced draw calls.
pub struct InstancedRectPipeline {
    pipeline: RenderPipeline,
    /// Per-instance vertex buffer; grows monotonically.
    instance_buffer: Buffer,
    /// Capacity of `instance_buffer` in bytes.
    instance_capacity_bytes: u64,
    /// `true` once we've warn-logged about hitting [`WARN_INSTANCE_CAP`];
    /// keeps the log to one line per session.
    warned_over_cap: bool,
}

impl InstancedRectPipeline {
    /// Build the pipeline. `viewport_bgl` is shared across all instanced
    /// pipelines and constructed by the `Renderer` (Phase 7) — see
    /// [`crate::pipeline_shared::viewport_bind_group_layout`].
    pub fn new(
        device: &Device,
        surface_format: TextureFormat,
        viewport_bgl: &BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("instanced_rect.wgsl"),
            source: ShaderSource::Wgsl(include_str!("shaders/instanced_rect.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("instanced-rect-pipeline-layout"),
            bind_group_layouts: &[Some(viewport_bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("instanced-rect-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    // Buffer 0: per-vertex unit quad ([f32; 2] × 6).
                    VertexBufferLayout {
                        array_stride: mem::size_of::<[f32; 2]>() as BufferAddress,
                        step_mode: VertexStepMode::Vertex,
                        attributes: &VERTEX_ATTRS,
                    },
                    // Buffer 1: per-instance RectInstance.
                    VertexBufferLayout {
                        array_stride: mem::size_of::<RectInstance>() as BufferAddress,
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
                    // Phase 1 premultiplied contract: One/OneMinusSrcAlpha
                    // on BOTH color and alpha.
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

        let instance_capacity_bytes = MIN_INSTANCES * mem::size_of::<RectInstance>() as u64;
        let instance_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("rect-instances"),
            size: instance_capacity_bytes,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            instance_buffer,
            instance_capacity_bytes,
            warned_over_cap: false,
        }
    }

    /// Upload `instances` to the per-instance buffer. Call once per frame
    /// **before** `begin_render_pass` (red-team P0-2: avoids dropping the
    /// old buffer mid-pass).
    pub fn prepare(&mut self, device: &Device, queue: &Queue, instances: &[RectInstance]) {
        if instances.is_empty() {
            return;
        }
        if instances.len() > WARN_INSTANCE_CAP && !self.warned_over_cap {
            log::warn!(
                "InstancedRectPipeline: {} instances exceeds soft cap {}",
                instances.len(),
                WARN_INSTANCE_CAP,
            );
            self.warned_over_cap = true;
        }
        self.ensure_capacity(device, instances.len());
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));
    }

    /// Record one instanced draw covering `range` of the buffer uploaded in
    /// the matching `prepare` call. Safe to call multiple times per pass
    /// with disjoint ranges (one per [`crate::Layer`]).
    ///
    /// All `&'a` borrows must outlive `pass`; the lifetimes prevent
    /// `set_vertex_buffer` from dangling if `instance_buffer` were
    /// reassigned mid-pass (it can't be — `prepare` is `&mut self`, `record`
    /// is `&self`).
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
        let stride = mem::size_of::<RectInstance>() as BufferAddress;
        let byte_range = (range.start as BufferAddress * stride)
            ..(range.end as BufferAddress * stride);

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, viewport_bg, &[]);
        pass.set_vertex_buffer(0, unit_quad.slice(..));
        pass.set_vertex_buffer(1, self.instance_buffer.slice(byte_range));
        pass.draw(0..6, 0..(range.end - range.start));
    }

    /// Current capacity in bytes — exposed for tests / observability.
    pub fn capacity_bytes(&self) -> u64 {
        self.instance_capacity_bytes
    }

    fn ensure_capacity(&mut self, device: &Device, n: usize) {
        let stride = mem::size_of::<RectInstance>() as u64;
        let needed = n as u64 * stride;
        if needed <= self.instance_capacity_bytes {
            return;
        }
        let min = MIN_INSTANCES * stride;
        let new_cap = needed.next_power_of_two().max(min);
        self.instance_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("rect-instances"),
            size: new_cap,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity_bytes = new_cap;
    }
}
