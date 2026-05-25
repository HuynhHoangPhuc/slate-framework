//! Glyph pipeline wiring: ctor seam used by [`Renderer::new`].
//!
//! Submodule for `Renderer`. The pipeline itself lives in
//! [`crate::glyph_pipeline`]; this file owns only the construction call site
//! so the parent ctor stays focused. Glyph pipeline needs the glyph atlas
//! bind group, so the atlas reference is part of the signature.

use wgpu::{BindGroupLayout, Device, TextureFormat};

use crate::atlas::Atlas;
use crate::glyph_pipeline::GlyphPipeline;

pub(super) fn init(
    device: &Device,
    format: TextureFormat,
    viewport_bgl: &BindGroupLayout,
    glyph_atlas: &Atlas,
) -> GlyphPipeline {
    GlyphPipeline::new(device, format, viewport_bgl, glyph_atlas)
}
