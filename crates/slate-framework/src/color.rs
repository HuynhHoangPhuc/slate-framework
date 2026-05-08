//! Color utilities and constants.
//!
//! Provides `Color` type with hex parsing and common constants.
//! Colors are stored as linear, premultiplied RGBA `[f32; 4]`.

use slate_renderer::srgb_u8_to_linear_premul;

/// RGBA color in linear, premultiplied format.
///
/// All color operations produce colors suitable for direct use in rendering.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Color(pub [f32; 4]);

impl Color {
    /// Transparent (fully invisible).
    pub const TRANSPARENT: Self = Self([0.0, 0.0, 0.0, 0.0]);

    /// White (#FFFFFF).
    pub const WHITE: Self = Self([1.0, 1.0, 1.0, 1.0]);

    /// Black (#000000).
    pub const BLACK: Self = Self([0.0, 0.0, 0.0, 1.0]);

    /// Red (#FF0000).
    pub const RED: Self = Self([1.0, 0.0, 0.0, 1.0]);

    /// Green (#00FF00).
    pub const GREEN: Self = Self([0.0, 1.0, 0.0, 1.0]);

    /// Blue (#0000FF).
    pub const BLUE: Self = Self([0.0, 0.0, 1.0, 1.0]);

    /// Create a color from sRGB u8 components.
    ///
    /// Converts to linear, premultiplied RGBA.
    pub fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self(srgb_u8_to_linear_premul([r, g, b, a]))
    }

    /// Create an opaque color from sRGB u8 components.
    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self::from_rgba(r, g, b, 255)
    }

    /// Parse a hex color string.
    ///
    /// Supports formats:
    /// - `#RGB` (e.g., `#F00` for red)
    /// - `#RRGGBB` (e.g., `#FF0000` for red)
    /// - `#RRGGBBAA` (e.g., `#FF0000FF` for opaque red)
    ///
    /// Returns `None` if parsing fails.
    pub fn from_hex(hex: &str) -> Option<Self> {
        let hex = hex.trim_start_matches('#');

        match hex.len() {
            // #RGB -> expand to #RRGGBB
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
                Some(Self::from_rgb(r * 17, g * 17, b * 17))
            }
            // #RRGGBB
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Self::from_rgb(r, g, b))
            }
            // #RRGGBBAA
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()?;
                Some(Self::from_rgba(r, g, b, a))
            }
            _ => None,
        }
    }

    /// Get the raw linear premultiplied RGBA array.
    pub fn as_array(&self) -> [f32; 4] {
        self.0
    }

    /// Create a color with modified alpha.
    pub fn with_alpha(mut self, alpha: f32) -> Self {
        // Unpremultiply, change alpha, repremultiply
        if self.0[3] > 0.0 {
            let inv_a = 1.0 / self.0[3];
            self.0[0] = self.0[0] * inv_a * alpha;
            self.0[1] = self.0[1] * inv_a * alpha;
            self.0[2] = self.0[2] * inv_a * alpha;
        }
        self.0[3] = alpha;
        self
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}

impl From<Color> for [f32; 4] {
    fn from(color: Color) -> Self {
        color.0
    }
}

impl From<[f32; 4]> for Color {
    fn from(arr: [f32; 4]) -> Self {
        Self(arr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_from_hex_6() {
        let red = Color::from_hex("#FF0000").unwrap();
        assert_eq!(red.0[3], 1.0); // opaque
        assert!(red.0[0] > 0.9); // red channel high
        assert!(red.0[1] < 0.01); // green channel low
        assert!(red.0[2] < 0.01); // blue channel low
    }

    #[test]
    fn color_from_hex_3() {
        let red = Color::from_hex("#F00").unwrap();
        assert!(red.0[0] > 0.9);
    }

    #[test]
    fn color_from_hex_8() {
        let half_red = Color::from_hex("#FF000080").unwrap();
        assert!((half_red.0[3] - 0.5).abs() < 0.1); // ~50% alpha
    }

    #[test]
    fn color_constants() {
        assert_eq!(Color::WHITE.0, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(Color::BLACK.0, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(Color::TRANSPARENT.0, [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn color_invalid_hex() {
        assert!(Color::from_hex("invalid").is_none());
        assert!(Color::from_hex("#GGG").is_none());
        assert!(Color::from_hex("#12345").is_none());
    }
}
