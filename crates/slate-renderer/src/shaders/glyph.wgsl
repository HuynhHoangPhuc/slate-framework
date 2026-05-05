// WGSL atlas-sampled glyph shader (instanced) — slate-framework Phase 5.
//
// Vertex stage consumes:
//   * a static unit quad (vertex buffer 0, VertexStepMode::Vertex)
//   * a per-instance GlyphInstance (vertex buffer 1, VertexStepMode::Instance)
//
// Fragment stage samples the shared R8 alpha atlas (linear; alpha is not
// gamma-encoded), multiplies by the per-instance premultiplied ink color,
// and outputs premultiplied linear RGBA — paired with `One/OneMinusSrcAlpha`
// blend at pipeline-creation site.
//
// Color contract (Phase 1): atlas stores 8-bit alpha mask in `.r`. Ink color
// `in.color` is premultiplied linear RGBA. Output mask scales both rgb and
// alpha so the contract stays premultiplied.
//
// `sub_pixel_variant` is plumbed through but unused in Phase 1 — Phase 2
// glyph producer will pick which of 4 atlas slots to sample upstream and
// hand the corresponding `uv_rect` directly.

struct Viewport {
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;
@group(1) @binding(0) var glyph_tex: texture_2d<f32>;
@group(1) @binding(1) var glyph_smp: sampler;

struct VsIn {
    /// Unit-quad vertex in [-1, 1] NDC (per-vertex, vertex buffer 0).
    @location(0) corner: vec2<f32>,
    /// Per-instance: [x, y, w, h] in logical pixels (top-left origin).
    @location(1) rect: vec4<f32>,
    /// Per-instance: [u0, v0, u1, v1] atlas-normalized UV rect.
    @location(2) uv_rect: vec4<f32>,
    /// Per-instance: linear premultiplied RGBA ink color.
    @location(3) color: vec4<f32>,
    /// Per-instance: sub-pixel positioning variant (0..4). Unused in Phase 1
    /// fragment shader; reserved for Phase 2 producer.
    @location(4) sub_pixel_variant: u32,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let t = in.corner * 0.5 + 0.5;
    let pixel_pos = in.rect.xy + in.rect.zw * t;

    let ndc = vec2(
        (pixel_pos.x / viewport.size.x) * 2.0 - 1.0,
        1.0 - (pixel_pos.y / viewport.size.y) * 2.0,
    );

    var out: VsOut;
    out.clip_position = vec4(ndc, 0.0, 1.0);
    out.uv = mix(in.uv_rect.xy, in.uv_rect.zw, t);
    // Multiply by zero anchors `sub_pixel_variant` as a real read so naga
    // never dead-strips the @location(4) binding (red-team M2 / code-review
    // H1: an unread vertex input is spec-allowed to be eliminated, breaking
    // pipeline creation on a future naga revision). Phase 2 producer will
    // replace the trivial use with real variant selection.
    out.color = in.color + vec4<f32>(0.0) * f32(in.sub_pixel_variant);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // R8Unorm: linear alpha mask in `.r`.
    let mask = textureSample(glyph_tex, glyph_smp, in.uv).r;
    // `in.color` is premultiplied; scaling by `mask` keeps it premultiplied.
    return in.color * mask;
}
