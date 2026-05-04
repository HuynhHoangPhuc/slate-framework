//! GPU pipeline for drawing a single antialiased rounded rectangle via SDF.
//!
//! # Usage
//!
//! ```ignore
//! // Build once, typically at startup.
//! let pipeline = RectPipeline::new(renderer.device(), renderer.surface_format());
//!
//! // Each frame: upload uniform, then record into a render pass.
//! pipeline.upload(renderer.queue(), RectUniform {
//!     center: [400.0, 300.0],
//!     half_size: [100.0, 60.0],
//!     color: [0.2, 0.5, 1.0, 1.0],
//!     viewport_size: [800.0, 600.0],
//!     corner_radius: 12.0,
//!     _pad: 0.0,
//! });
//! pipeline.record(&mut render_pass);
//! ```

use std::mem;

use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BlendComponent, BlendFactor,
    BlendOperation, BlendState, Buffer, BufferBindingType, BufferDescriptor, BufferUsages,
    ColorTargetState, ColorWrites, Device, FragmentState, MultisampleState,
    PipelineLayoutDescriptor, PrimitiveState, PrimitiveTopology, Queue, RenderPass,
    RenderPipeline, RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages,
    TextureFormat, VertexState,
};

// ----- Uniform data -----

/// CPU-side mirror of the `RectUniform` struct in `shaders/rect.wgsl`.
///
/// Layout (std140-compatible, 16-byte aligned, 48 bytes total):
///
/// | Field           | Offset | Size | WGSL type    |
/// |-----------------|--------|------|--------------|
/// | `center`        |      0 |    8 | `vec2<f32>`  |
/// | `half_size`     |      8 |    8 | `vec2<f32>`  |
/// | `color`         |     16 |   16 | `vec4<f32>`  |
/// | `viewport_size` |     32 |    8 | `vec2<f32>`  |
/// | `corner_radius` |     40 |    4 | `f32`        |
/// | `_pad`          |     44 |    4 | `f32`        |
///
/// The padding at offset 44 is explicit so that `size_of::<RectUniform>() == 48`
/// and the struct alignment matches the WGSL `uniform` address-space requirement.
#[repr(C)]
#[derive(Copy, Clone, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RectUniform {
    /// Pixel-space center of the rectangle.
    pub center: [f32; 2],
    /// Half-width and half-height in pixels.
    pub half_size: [f32; 2],
    /// RGBA color in linear sRGB (alpha is straight, not premultiplied).
    pub color: [f32; 4],
    /// Physical-pixel viewport dimensions used by the vertex shader for NDC → pixel conversion.
    pub viewport_size: [f32; 2],
    /// Corner radius in pixels (0 = sharp corners).
    pub corner_radius: f32,
    /// Explicit padding — keep zero; required for WGSL std140 struct alignment.
    pub _pad: f32,
}

// Compile-time layout assertion: struct must be exactly 48 bytes.
const _: () = assert!(mem::size_of::<RectUniform>() == 48);

// ----- Pipeline -----

/// Encapsulates the wgpu render pipeline, uniform buffer, and bind group
/// for the SDF rounded-rectangle shader (`shaders/rect.wgsl`).
///
/// Construct with [`RectPipeline::new`], then per-frame:
/// 1. Call [`RectPipeline::upload`] to write `RectUniform` data to the GPU.
/// 2. Call [`RectPipeline::record`] inside an active `RenderPass` to set the
///    pipeline, bind group, and issue the 6-vertex draw call.
pub struct RectPipeline {
    pipeline: RenderPipeline,
    uniform_buffer: Buffer,
    bind_group: BindGroup,
    // Kept for potential future re-creation (e.g. format change).
    #[allow(dead_code)]
    bind_group_layout: BindGroupLayout,
}

impl RectPipeline {
    /// Create the render pipeline for the given surface `format`.
    ///
    /// This compiles the WGSL shader, allocates a 48-byte uniform buffer,
    /// and wires everything into a `BindGroup`.  Typically called once at
    /// startup after the wgpu `Device` is available.
    pub fn new(device: &Device, format: TextureFormat) -> Self {
        // --- Shader module ---
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("rect.wgsl"),
            source: ShaderSource::Wgsl(include_str!("shaders/rect.wgsl").into()),
        });

        // --- Bind group layout: single uniform buffer at binding 0 ---
        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("rect-bgl"),
            entries: &[BindGroupLayoutEntry {
                binding: 0,
                // Both VS (viewport_size) and FS (color, SDF params) read the uniform.
                visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
                ty: BindingType::Buffer {
                    ty: BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // --- Pipeline layout ---
        // wgpu 29: bind_group_layouts is &[Option<&BindGroupLayout>]; no push_constant_ranges.
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("rect-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        // --- Render pipeline ---
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("rect-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                // No vertex buffer — positions are baked into the shader via vertex_index.
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState {
                        color: BlendComponent {
                            src_factor: BlendFactor::SrcAlpha,
                            dst_factor: BlendFactor::OneMinusSrcAlpha,
                            operation: BlendOperation::Add,
                        },
                        // Standard "over" compositing for alpha channel.
                        alpha: BlendComponent::OVER,
                    }),
                    write_mask: ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: PrimitiveState {
                topology: PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            // No depth or stencil — UI rendering uses painter's algorithm ordering.
            depth_stencil: None,
            multisample: MultisampleState::default(),
            // wgpu 29: field is `multiview_mask`, not `multiview`.
            multiview_mask: None,
            cache: None,
        });

        // --- Uniform buffer (write-only from CPU, uniform from GPU) ---
        let uniform_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("rect-uniform-buf"),
            size: mem::size_of::<RectUniform>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // --- Bind group ---
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("rect-bg"),
            layout: &bind_group_layout,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(uniform_buffer.as_entire_buffer_binding()),
            }],
        });

        Self {
            pipeline,
            uniform_buffer,
            bind_group,
            bind_group_layout,
        }
    }

    /// Write `uniform` data to the GPU uniform buffer.
    ///
    /// Call this once per frame (or whenever the rect parameters change)
    /// before calling [`RectPipeline::record`].
    pub fn upload(&self, queue: &Queue, uniform: RectUniform) {
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    /// Record the pipeline draw call into an active `RenderPass`.
    ///
    /// Sets the pipeline, binds the uniform buffer at group 0, and issues
    /// a 6-vertex draw (2 triangles = full-screen quad, no vertex buffer).
    /// The WGSL fragment shader discards pixels outside the SDF boundary.
    /// Record the pipeline draw call into an active `RenderPass`.
    ///
    /// `&self` and the render pass lifetime are independent: `set_pipeline` and
    /// `set_bind_group` in wgpu 29 do not bind the resource lifetime to `'pass`.
    pub fn record(&self, rpass: &mut RenderPass<'_>) {
        rpass.set_pipeline(&self.pipeline);
        rpass.set_bind_group(0, &self.bind_group, &[]);
        rpass.draw(0..6, 0..1);
    }
}
