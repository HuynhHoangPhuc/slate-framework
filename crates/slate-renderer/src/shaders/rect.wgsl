// WGSL SDF rounded-rectangle shader — slate-framework Phase 5
//
// Vertex shader emits a full-screen quad (6 vertices, 2 triangles) via
// vertex_index; no vertex buffer is bound.  The fragment shader evaluates
// a 2-D signed distance function for a rounded rectangle and applies a
// 1-pixel AA band via clamp(0.5 - d, 0, 1).
//
// Uniform layout (std140 / 16-byte aligned, 48 bytes total):
//   offset  0: center        vec2<f32>   — pixel-space rect center
//   offset  8: half_size     vec2<f32>   — half-width / half-height in pixels
//   offset 16: color         vec4<f32>   — RGBA, sRGB-linear
//   offset 32: viewport_size vec2<f32>   — physical-pixel viewport dimensions
//   offset 40: corner_radius f32         — corner radius in pixels
//   offset 44: _pad          f32         — explicit padding; struct size = 48

struct RectUniform {
    /// Pixel-space center of rect — offset 0
    center: vec2<f32>,
    /// Half width / half height in pixels — offset 8
    half_size: vec2<f32>,
    /// RGBA color, sRGB premultiplied if needed — offset 16 (16-byte aligned)
    color: vec4<f32>,
    /// Viewport size in physical pixels — offset 32
    viewport_size: vec2<f32>,
    /// Corner radius in pixels — offset 40
    corner_radius: f32,
    /// Padding to reach 48-byte / 16-byte-aligned struct — offset 44
    _pad: f32,
};

@group(0) @binding(0) var<uniform> rect: RectUniform;

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) pixel_position: vec2<f32>,
};

// Full-screen quad as 2 triangles via vertex_index lookup.
// No vertex buffer; positions are baked into the shader.
@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VsOut {
    var positions = array<vec2<f32>, 6>(
        vec2(-1.0, -1.0), vec2( 1.0, -1.0), vec2(-1.0,  1.0),
        vec2( 1.0, -1.0), vec2( 1.0,  1.0), vec2(-1.0,  1.0),
    );
    let ndc = positions[idx];
    var out: VsOut;
    out.clip_position = vec4(ndc, 0.0, 1.0);
    // Convert NDC [-1, 1] → pixel space [0, viewport].
    // Y is flipped: NDC +Y is up, pixel +Y is down.
    out.pixel_position = vec2(
        (ndc.x * 0.5 + 0.5) * rect.viewport_size.x,
        (1.0 - (ndc.y * 0.5 + 0.5)) * rect.viewport_size.y,
    );
    return out;
}

// 2-D SDF for a rounded rectangle centered at the origin.
//
// p         — pixel position relative to rect center
// half_size — (half_width, half_height) in pixels
// radius    — corner radius in pixels
//
// Returns negative inside, zero on edge, positive outside.
fn sdf_rounded_rect(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2(radius, radius);
    return length(max(q, vec2(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Translate to rect-local coordinates (center at origin).
    let local = in.pixel_position - rect.center;
    let d = sdf_rounded_rect(local, rect.half_size, rect.corner_radius);
    // 1-pixel AA band: alpha = 1 deep inside, 0 outside, smooth crossing at edge.
    let alpha = clamp(0.5 - d, 0.0, 1.0);
    if alpha <= 0.0 {
        discard;
    }
    return vec4(rect.color.rgb, rect.color.a * alpha);
}
