//! Shared GPU resources for instanced primitive pipelines.
//!
//! Phase 1/2 scope: viewport uniform layout (used by every instance pipeline
//! to convert pixel-space → NDC) and the static unit-quad vertex buffer
//! (each instance pipeline draws an expanded copy of this quad).
//!
//! Per plan §Architecture (red-team P1-11), `Renderer::new` (Phase 7) builds
//! the BGL + buffer once and hands them by reference into each pipeline's
//! `new`/`record`. Until Phase 7 lands, the test harness builds them inline.

use std::mem;
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use wgpu::{
    BindGroupLayout, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingType, Buffer,
    BufferBindingType, BufferDescriptor, BufferUsages, Device, ShaderStages,
};

/// CPU mirror of WGSL `Viewport { size: vec2<f32>, _pad: vec2<f32> }`.
///
/// 16 bytes — meets the uniform-buffer minimum binding size.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
pub struct ViewportUniform {
    /// Physical-pixel viewport `[width, height]`.
    pub size: [f32; 2],
    /// Padding to 16 bytes (uniform-address-space alignment).
    pub _pad: [f32; 2],
}
const _: () = assert!(mem::size_of::<ViewportUniform>() == 16);

/// Bind-group layout for the viewport uniform: single VS+FS uniform at binding 0.
///
/// Constructed once and shared by every instanced primitive pipeline (rect,
/// shadow, image, glyph). Phase 7's `Renderer` owns the resulting `BindGroup`.
pub fn viewport_bind_group_layout(device: &Device) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label: Some("slate-viewport-bgl"),
        entries: &[BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::VERTEX | ShaderStages::FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                // Free validation: BGL-time check that bound buffer is ≥16 B.
                min_binding_size: NonZeroU64::new(mem::size_of::<ViewportUniform>() as u64),
            },
            count: None,
        }],
    })
}

/// Two triangles in `[-1, 1]` NDC, 6 vertices, one `[f32; 2]` per vertex.
///
/// Each instanced pipeline binds this at vertex buffer slot 0 and reads
/// `corner` at `@location(0)`. Index buffer is omitted — 6 verts is below
/// the index-overhead break-even.
pub fn create_unit_quad(device: &Device) -> Buffer {
    // Same winding as `rect.wgsl::vs_main` (CCW).
    const QUAD: [[f32; 2]; 6] = [
        [-1.0, -1.0],
        [1.0, -1.0],
        [-1.0, 1.0],
        [1.0, -1.0],
        [1.0, 1.0],
        [-1.0, 1.0],
    ];
    let bytes: &[u8] = bytemuck::cast_slice(&QUAD);
    let buffer = device.create_buffer(&BufferDescriptor {
        label: Some("slate-unit-quad"),
        size: bytes.len() as u64,
        usage: BufferUsages::VERTEX,
        mapped_at_creation: true,
    });
    buffer.slice(..).get_mapped_range_mut().copy_from_slice(bytes);
    buffer.unmap();
    buffer
}
