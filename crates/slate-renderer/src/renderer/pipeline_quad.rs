//! Instanced-rect pipeline wiring: ctor seam used by [`Renderer::new`].
//!
//! Submodule for `Renderer`. The pipeline itself lives in
//! [`crate::instanced_rect_pipeline`]; this file owns only the construction
//! call site so the parent ctor stays focused.

use wgpu::{BindGroupLayout, Device, TextureFormat};

use crate::instanced_rect_pipeline::InstancedRectPipeline;

pub(super) fn init(
    device: &Device,
    format: TextureFormat,
    viewport_bgl: &BindGroupLayout,
) -> InstancedRectPipeline {
    InstancedRectPipeline::new(device, format, viewport_bgl)
}
