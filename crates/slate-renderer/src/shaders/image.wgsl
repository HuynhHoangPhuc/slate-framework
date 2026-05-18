// WGSL atlas-sampled image shader (instanced) — slate-framework Phase 4.
//
// Vertex stage consumes:
//   * a static unit quad (vertex buffer 0, VertexStepMode::Vertex)
//   * a per-instance ImageInstance (vertex buffer 1, VertexStepMode::Instance)
//
// Fragment stage samples the shared color atlas (Rgba8UnormSrgb → hardware
// sRGB→linear decode), explicitly premultiplies the (straight) atlas texel,
// then multiplies by the premultiplied tint. Output is premultiplied linear
// RGBA — paired with `One/OneMinusSrcAlpha` blend at pipeline-creation site.
//
// Color contract (Phase 1): atlas RGB is decoded sRGB→linear by hardware on
// sample; alpha is linear pass-through. PNGs from `image::open()` are straight
// RGBA, so we premultiply at sample time. `tint` is supplied premultiplied
// (white = `[1, 1, 1, 1]`).

struct Viewport {
    /// Logical-pixel (lpx) viewport size; `pixel_pos / viewport.size` NDC
    /// mapping is scale-invariant when both sides agree on units.
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;
@group(1) @binding(0) var atlas_tex: texture_2d<f32>;
@group(1) @binding(1) var atlas_smp: sampler;

struct VsIn {
    /// Unit-quad vertex in [-1, 1] NDC (per-vertex, vertex buffer 0).
    @location(0) corner: vec2<f32>,
    /// Per-instance: [x, y, w, h] in logical pixels (top-left origin).
    @location(1) rect: vec4<f32>,
    /// Per-instance: [u0, v0, u1, v1] atlas-normalized UV rect.
    @location(2) uv_rect: vec4<f32>,
    /// Per-instance: linear premultiplied RGBA tint multiplier.
    @location(3) tint: vec4<f32>,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
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
    out.tint = in.tint;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Atlas is Rgba8UnormSrgb: hw decodes RGB sRGB→linear; A is linear.
    let texel = textureSample(atlas_tex, atlas_smp, in.uv);
    // Atlas stores straight RGBA (image::open() default); premultiply now.
    let texel_premul = vec4(texel.rgb * texel.a, texel.a);
    return texel_premul * in.tint;
}
