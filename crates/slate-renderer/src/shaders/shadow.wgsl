// Gaussian rounded-rect shadow shader — slate-framework Phase 6
//
// Direct port of Zed's production shadow algorithm from
// `zed/crates/gpui_wgpu/src/shaders.wgsl:319-1020`.
//
// Algorithm: closed-form X erf integral (blur_along_x) that bakes the
// rounded-corner curve into its bounds, weighted by 4 Gaussian Y-samples.
// No SDF helper — the corner is encoded inside blur_along_x's `curved` term.
//
// Color contract: output is **linear, premultiplied** RGBA.
// Blend state: One/OneMinusSrcAlpha (both color and alpha).

struct Viewport {
    size: vec2<f32>,
    _pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;

struct VsIn {
    @location(0) corner: vec2<f32>,
    @location(1) rect: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) corner_radius: f32,
    @location(4) blur_radius: f32,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) local_pos: vec2<f32>,
    @location(1) rect_size: vec2<f32>,
    @location(2) corner_radius: f32,
    @location(3) blur_radius: f32,
    @location(4) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let half_size = in.rect.zw * 0.5;
    let center = in.rect.xy + half_size;

    // Inflate quad by ±3σ to capture 99.7% of the Gaussian tail.
    let inflate = 3.0 * in.blur_radius;
    let inflated_half = half_size + vec2(inflate, inflate);

    // Map unit-quad corner [-1, 1] to inflated pixel position.
    let t = in.corner * 0.5 + 0.5;
    let pixel_pos = (center - inflated_half) + (inflated_half * 2.0) * t;

    // Pixel → NDC. +Y down in pixel space, +Y up in NDC.
    let ndc = vec2(
        (pixel_pos.x / viewport.size.x) * 2.0 - 1.0,
        1.0 - (pixel_pos.y / viewport.size.y) * 2.0,
    );

    var out: VsOut;
    out.clip_position = vec4(ndc, 0.0, 1.0);
    out.local_pos = pixel_pos - center;
    out.rect_size = in.rect.zw;
    out.corner_radius = in.corner_radius;
    out.blur_radius = in.blur_radius;
    out.color = in.color;
    return out;
}

// Standard Gaussian density for weighting Y-axis samples.
fn gaussian(x: f32, sigma: f32) -> f32 {
    return exp(-(x * x) / (2.0 * sigma * sigma)) / (sqrt(2.0 * 3.141592653589793) * sigma);
}

// erf approximation — A&S 7.1.25, vec2-vectorised, max error ~5e-4.
// Verbatim from Zed (gpui_wgpu/src/shaders.wgsl:325-331).
fn erf(v: vec2<f32>) -> vec2<f32> {
    let s = sign(v);
    let a = abs(v);
    let r1 = 1.0 + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    let r2 = r1 * r1;
    return s - s / (r2 * r2);
}

// Integral along X of the (corner-clipped) rect at offset y from center.
// The `curved` term encodes the circular-corner clip: near the top/bottom
// edge, the effective half-width shrinks by the corner curve.
// Verbatim from Zed (gpui_wgpu/src/shaders.wgsl:333-338).
fn blur_along_x(x: f32, y: f32, sigma: f32, corner: f32, half_size: vec2<f32>) -> f32 {
    let delta = min(half_size.y - corner - abs(y), 0.0);
    let curved = half_size.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    let integral = 0.5 + 0.5 * erf((x + vec2(-curved, curved)) * (sqrt(0.5) / sigma));
    return integral.y - integral.x;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let half_size = in.rect_size * 0.5;
    let point = in.local_pos;
    let sigma = in.blur_radius;
    let corner = in.corner_radius;

    // Sample range clamped to rect's Y extent (Zed pattern shaders.wgsl:1003-1006).
    let low = point.y - half_size.y;
    let high = point.y + half_size.y;
    let start = clamp(-3.0 * sigma, low, high);
    let end = clamp(3.0 * sigma, low, high);

    // 4 uniform Y-samples weighted by Gaussian density.
    let step = (end - start) / 4.0;
    var y = start + step * 0.5;
    var alpha = 0.0;
    for (var i = 0; i < 4; i = i + 1) {
        alpha = alpha + blur_along_x(point.x, point.y - y, sigma, corner, half_size) * gaussian(y, sigma) * step;
        y = y + step;
    }
    alpha = clamp(alpha, 0.0, 1.0);

    // Premultiplied output (Phase 1 contract).
    return vec4(in.color.rgb * alpha, in.color.a * alpha);
}
