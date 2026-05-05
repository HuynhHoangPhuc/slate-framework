//! Gaussian rounded-rect shadow pipeline (Phase 6).
//!
//! Single-pass analytical shadow using Zed's `blur_along_x` algorithm —
//! no atlas, no FBO, no multi-pass. Quad inflated by ±3σ in the vertex stage.
//!
//! # Two-phase API (red-team P0-2 fix)
//!
//! 1. [`prepare`] — uploads instance data BEFORE `RenderPass` starts.
//! 2. [`record`] — issues draw commands inside the pass (immutable self).
//!
//! # Color contract
//!
//! `ShadowInstance.color` is **linear, premultiplied** RGBA. Blend state is
//! `One/OneMinusSrcAlpha` on both color and alpha.
//!
//! [`prepare`]: ShadowPipeline::prepare
//! [`record`]: ShadowPipeline::record

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

use crate::scene::ShadowInstance;

const MIN_INSTANCES: u64 = 64;
const WARN_INSTANCE_CAP: usize = 1_000_000;

/// Vertex attributes for `ShadowInstance` (48 bytes):
/// - location 1: rect [f32; 4] (offset 0)
/// - location 2: color [f32; 4] (offset 16)
/// - location 3: corner_radius f32 (offset 32)
/// - location 4: blur_radius f32 (offset 36)
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
        format: VertexFormat::Float32,
        offset: 32,
        shader_location: 3,
    },
    VertexAttribute {
        format: VertexFormat::Float32,
        offset: 36,
        shader_location: 4,
    },
];

const VERTEX_ATTRS: [VertexAttribute; 1] = [VertexAttribute {
    format: VertexFormat::Float32x2,
    offset: 0,
    shader_location: 0,
}];

/// GPU pipeline for Gaussian rounded-rect shadows via instanced draw calls.
pub struct ShadowPipeline {
    pipeline: RenderPipeline,
    instance_buffer: Buffer,
    instance_capacity_bytes: u64,
    last_instance_count: u32,
    warned_over_cap: bool,
}

impl ShadowPipeline {
    pub fn new(
        device: &Device,
        surface_format: TextureFormat,
        viewport_bgl: &BindGroupLayout,
    ) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("shadow.wgsl"),
            source: ShaderSource::Wgsl(include_str!("shaders/shadow.wgsl").into()),
        });

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("shadow-pipeline-layout"),
            bind_group_layouts: &[Some(viewport_bgl)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("shadow-pipeline"),
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
                        array_stride: mem::size_of::<ShadowInstance>() as BufferAddress,
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

        let instance_capacity_bytes = MIN_INSTANCES * mem::size_of::<ShadowInstance>() as u64;
        let instance_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("shadow-instances"),
            size: instance_capacity_bytes,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            pipeline,
            instance_buffer,
            instance_capacity_bytes,
            last_instance_count: 0,
            warned_over_cap: false,
        }
    }

    /// Upload instances to the buffer. Call BEFORE `begin_render_pass`.
    pub fn prepare(&mut self, device: &Device, queue: &Queue, instances: &[ShadowInstance]) {
        if instances.is_empty() {
            self.last_instance_count = 0;
            return;
        }
        if instances.len() > WARN_INSTANCE_CAP && !self.warned_over_cap {
            log::warn!(
                "ShadowPipeline: {} instances exceeds soft cap {}",
                instances.len(),
                WARN_INSTANCE_CAP,
            );
            self.warned_over_cap = true;
        }
        self.ensure_capacity(device, instances.len());
        queue.write_buffer(&self.instance_buffer, 0, bytemuck::cast_slice(instances));
        self.last_instance_count = instances.len() as u32;
    }

    /// Record draw commands for `range` of the prepared buffer.
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
        assert!(
            range.end <= self.last_instance_count,
            "ShadowPipeline::record: range {:?} extends past last_instance_count={}",
            range,
            self.last_instance_count,
        );
        let stride = mem::size_of::<ShadowInstance>() as BufferAddress;
        let byte_range =
            (range.start as BufferAddress * stride)..(range.end as BufferAddress * stride);

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, viewport_bg, &[]);
        pass.set_vertex_buffer(0, unit_quad.slice(..));
        pass.set_vertex_buffer(1, self.instance_buffer.slice(byte_range));
        pass.draw(0..6, 0..(range.end - range.start));
    }

    pub fn capacity_bytes(&self) -> u64 {
        self.instance_capacity_bytes
    }

    fn ensure_capacity(&mut self, device: &Device, n: usize) {
        let stride = mem::size_of::<ShadowInstance>() as u64;
        let needed = (n as u64)
            .checked_mul(stride)
            .expect("instance count × stride overflows u64");
        if needed <= self.instance_capacity_bytes {
            return;
        }
        let min = MIN_INSTANCES * stride;
        let new_cap = needed.next_power_of_two().max(min);
        self.instance_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("shadow-instances"),
            size: new_cap,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity_bytes = new_cap;
    }
}
