//! Color-space conversion helpers.
//!
//! The renderer is configured with a `*_Srgb` surface format, which means the
//! GPU performs an automatic linear → sRGB encoding on every fragment-shader
//! write. Callers therefore feed **linear** color values into [`RectUniform`]
//! (and any future material struct).
//!
//! Most artwork pipelines deliver colors in sRGB-encoded form (CSS hex, Figma,
//! design tokens, …). Use these helpers to convert at the boundary so the
//! intent of "I want #66ccff on screen" is preserved.
//!
//! [`RectUniform`]: crate::rect_pipeline::RectUniform

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
}
