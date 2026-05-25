//! Shadow pipeline wiring: ctor seam used by [`Renderer::new`].
//!
//! Submodule for `Renderer`. The pipeline itself lives in
//! [`crate::shadow_pipeline`]; this file owns only the construction call site
//! so the parent ctor stays focused.

use wgpu::{BindGroupLayout, Device, TextureFormat};

use crate::shadow_pipeline::ShadowPipeline;

pub(super) fn init(
    device: &Device,
    format: TextureFormat,
    viewport_bgl: &BindGroupLayout,
) -> ShadowPipeline {
    ShadowPipeline::new(device, format, viewport_bgl)
}
