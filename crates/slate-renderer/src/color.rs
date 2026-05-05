//! Color-space conversion helpers.
//!
//! The renderer is configured with a `*_Srgb` surface format, which means the
//! GPU performs an automatic linear → sRGB encoding on every fragment-shader
//! write. Callers therefore feed **linear** color values into instance structs
//! ([`RectInstance`], [`ShadowInstance`], etc.).
//!
//! Most artwork pipelines deliver colors in sRGB-encoded form (CSS hex, Figma,
//! design tokens, …). Use these helpers to convert at the boundary so the
//! intent of "I want #66ccff on screen" is preserved.
//!
//! [`RectInstance`]: crate::scene::RectInstance
//! [`ShadowInstance`]: crate::scene::ShadowInstance

/// Convert a single sRGB-encoded channel in `[0.0, 1.0]` to linear space.
///
/// Implements the IEC 61966-2-1 piecewise transfer function used by every
/// modern color stack (CSS, ICC sRGB profile, P3-D65 → linear, …).
pub fn srgb_channel_to_linear(c: f32) -> f32 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Convert an sRGB color expressed as four `f32` channels in `[0.0, 1.0]`
/// to the linear-space RGBA the renderer expects. Alpha is passed through.
pub fn srgb_to_linear(rgba: [f32; 4]) -> [f32; 4] {
    [
        srgb_channel_to_linear(rgba[0]),
        srgb_channel_to_linear(rgba[1]),
        srgb_channel_to_linear(rgba[2]),
        rgba[3],
    ]
}

/// Convert an 8-bit sRGB color (e.g. `#66ccff` → `[0x66, 0xcc, 0xff, 0xff]`)
/// to the linear-space RGBA the renderer expects.
pub fn srgb_u8_to_linear(rgba: [u8; 4]) -> [f32; 4] {
    let to_unit = |b: u8| b as f32 / 255.0;
    srgb_to_linear([
        to_unit(rgba[0]),
        to_unit(rgba[1]),
        to_unit(rgba[2]),
        to_unit(rgba[3]),
    ])
}

/// Inverse of [`srgb_channel_to_linear`]: linear → sRGB-encoded float.
///
/// Mostly useful in tests that compute an expected pixel value in linear space
/// and need to compare against a readback from an sRGB surface (the GPU
/// performs this same encoding on store).
pub fn linear_to_srgb_channel(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Encode a linear `[f32; 4]` as 8-bit sRGB bytes (alpha passes through linear,
/// matching `Bgra8UnormSrgb` storage semantics — see `tests/common/mod.rs`).
pub fn linear_to_srgb_u8(rgba: [f32; 4]) -> [u8; 4] {
    let to_byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [
        to_byte(linear_to_srgb_channel(rgba[0])),
        to_byte(linear_to_srgb_channel(rgba[1])),
        to_byte(linear_to_srgb_channel(rgba[2])),
        to_byte(rgba[3]),
    ]
}

/// Convert an 8-bit sRGB color to **linear, premultiplied** RGBA.
///
/// This is the canonical input form for every Phase 1+ instance struct
/// (`RectInstance.color`, `ShadowInstance.color`, `ImageInstance.tint`,
/// `GlyphInstance.color`). The renderer's blend state is configured for
/// premultiplied source (`One/OneMinusSrcAlpha` on both color and alpha),
/// so feeding straight-alpha colors will produce visibly wrong compositing.
pub fn srgb_u8_to_linear_premul(rgba: [u8; 4]) -> [f32; 4] {
    let a = rgba[3] as f32 / 255.0;
    let r = srgb_channel_to_linear(rgba[0] as f32 / 255.0) * a;
    let g = srgb_channel_to_linear(rgba[1] as f32 / 255.0) * a;
    let b = srgb_channel_to_linear(rgba[2] as f32 / 255.0) * a;
    [r, g, b, a]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    #[test]
    fn srgb_endpoints_round_trip() {
        assert!(approx(srgb_channel_to_linear(0.0), 0.0));
        assert!(approx(srgb_channel_to_linear(1.0), 1.0));
    }

    #[test]
    fn hex_66ccff_matches_known_linear() {
        // #66ccff = (102, 204, 255). Known linear values from the sRGB transfer
        // function: ~(0.1329, 0.6038, 1.0).
        let lin = srgb_u8_to_linear([0x66, 0xcc, 0xff, 0xff]);
        assert!(approx(lin[0], 0.132_868));
        assert!(approx(lin[1], 0.603_827));
        assert!(approx(lin[2], 1.0));
        assert!(approx(lin[3], 1.0));
    }

    #[test]
    fn alpha_passes_through_unchanged() {
        let lin = srgb_to_linear([1.0, 1.0, 1.0, 0.5]);
        assert!(approx(lin[3], 0.5));
    }

    #[test]
    fn premul_red_half_alpha_matches_expected() {
        // sRGB (255,0,0,128): linear R = 1.0; premul R = 1.0 * (128/255) ≈ 0.502.
        let p = srgb_u8_to_linear_premul([255, 0, 0, 128]);
        assert!(approx(p[0], 0.501_961));
        assert!(approx(p[1], 0.0));
        assert!(approx(p[2], 0.0));
        assert!(approx(p[3], 0.501_961));
    }

    #[test]
    fn premul_opaque_matches_straight() {
        // Alpha = 255 → premul ≡ straight linear.
        let straight = srgb_u8_to_linear([0x66, 0xcc, 0xff, 0xff]);
        let premul = srgb_u8_to_linear_premul([0x66, 0xcc, 0xff, 0xff]);
        for i in 0..4 {
            assert!(approx(premul[i], straight[i]));
        }
    }

    #[test]
    fn premul_zero_alpha_zeros_color() {
        let p = srgb_u8_to_linear_premul([255, 255, 255, 0]);
        assert!(approx(p[0], 0.0));
        assert!(approx(p[3], 0.0));
    }
}
