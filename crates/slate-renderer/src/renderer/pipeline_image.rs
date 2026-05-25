//! Image pipeline wiring: ctor seam used by [`Renderer::new`].
//!
//! Submodule for `Renderer`. The pipeline itself lives in
//! [`crate::image_pipeline`]; this file owns only the construction call site
//! so the parent ctor stays focused. Image pipeline needs the image atlas
//! bind group, so the atlas reference is part of the signature.

use wgpu::{BindGroupLayout, Device, TextureFormat};

use crate::atlas::Atlas;
use crate::image_pipeline::ImagePipeline;

pub(super) fn init(
    device: &Device,
    format: TextureFormat,
    viewport_bgl: &BindGroupLayout,
    image_atlas: &Atlas,
) -> ImagePipeline {
    ImagePipeline::new(device, format, viewport_bgl, image_atlas)
}
