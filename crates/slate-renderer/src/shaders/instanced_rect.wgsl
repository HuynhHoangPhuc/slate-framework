// WGSL SDF rounded-rectangle shader (instanced) — slate-framework Phase 2
//
// Vertex stage consumes:
//   * a static unit quad (vertex buffer 0, VertexStepMode::Vertex)
//   * a per-instance RectInstance (vertex buffer 1, VertexStepMode::Instance)
//
// Fragment stage runs the same `sdf_rounded_rect` + 1-px AA band as
// the original Phase 0 single-rect uniform shader — math copied verbatim.
// The only structural change is the data path: per-instance vertex attributes
// instead of a uniform buffer.
//
// Color contract: `RectInstance.color` is **linear, premultiplied** RGBA.
// Blend state at pipeline-creation site is `One/OneMinusSrcAlpha` for both
// color and alpha; the fragment scales all four channels by AA coverage so
// the premul invariant (rgb ≤ a) is preserved.

struct Viewport {
    /// Physical-pixel viewport size. `_pad` keeps the struct 16-byte aligned
    /// (std140 requirement for uniform-address-space buffers).
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;

struct VsIn {
    /// Unit-quad vertex in [-1, 1] NDC (per-vertex, vertex buffer 0).
    @location(0) corner: vec2<f32>,
    /// Per-instance: [x, y, w, h] in logical pixels (top-left origin).
    @location(1) rect: vec4<f32>,
    /// Per-instance: linear RGBA, premultiplied.
    @location(2) color: vec4<f32>,
    /// Per-instance: corner radius in logical pixels.
    @location(3) corner_radius: f32,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    /// Position in rect-local pixel space (rect center at origin).
    @location(0) local_pos: vec2<f32>,
    /// Half-width / half-height in pixels (passed through for the SDF).
    @location(1) half_size: vec2<f32>,
    /// Corner radius in pixels.
    @location(2) corner_radius: f32,
    /// Premultiplied RGBA.
    @location(3) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    // Map unit-quad corner [-1, 1] → instance pixel position [rect.xy, rect.xy+rect.zw].
    let t = in.corner * 0.5 + 0.5;
    let pixel_pos = in.rect.xy + in.rect.zw * t;

    // Pixel → NDC. +Y is down in pixel space, +Y is up in NDC.
    let ndc = vec2(
        (pixel_pos.x / viewport.size.x) * 2.0 - 1.0,
        1.0 - (pixel_pos.y / viewport.size.y) * 2.0,
    );

    let half_size = in.rect.zw * 0.5;
    let center = in.rect.xy + half_size;

    var out: VsOut;
    out.clip_position = vec4(ndc, 0.0, 1.0);
    out.local_pos = pixel_pos - center;
    out.half_size = half_size;
    out.corner_radius = in.corner_radius;
    out.color = in.color;
    return out;
}

// SDF rounded-rect helper — kept inline (WGSL has no shared module).
fn sdf_rounded_rect(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2(radius, radius);
    return length(max(q, vec2(0.0, 0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let d = sdf_rounded_rect(in.local_pos, in.half_size, in.corner_radius);
    let coverage = clamp(0.5 - d, 0.0, 1.0);
    if coverage <= 0.0 {
        discard;
    }
    // Input `color` is premultiplied; scale all four channels by AA coverage
    // to preserve the premul invariant (rgb ≤ a).
    return in.color * coverage;
}
