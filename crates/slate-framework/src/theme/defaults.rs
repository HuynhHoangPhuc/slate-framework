//! Default light and dark themes shipped with the framework.
//!
//! Adopters override by cloning one of these and tweaking fields, then
//! installing via [`crate::App::theme`].

use super::tokens::{RADII, SPACING, TYPOGRAPHY, Theme};
use crate::color::Color;

/// sRGB opaque colour from a `#rrggbb` literal. A malformed literal is only
/// reachable via a typo in this file, so fail loudly in debug and fall back to
/// magenta in release.
fn hex(s: &str) -> Color {
    match Color::from_hex(s) {
        Some(c) => c,
        None => {
            debug_assert!(false, "malformed theme colour literal: {s}");
            Color([1.0, 0.0, 1.0, 1.0])
        }
    }
}

impl Theme {
    /// The default light theme.
    pub fn light() -> Self {
        Theme {
            bg: hex("#ffffff"),
            surface: hex("#f5f5f7"),
            fg: hex("#1d1d1f"),
            muted: hex("#6e6e73"),
            accent: hex("#0a84ff"),
            border: hex("#d2d2d7"),
            danger: hex("#ff3b30"),
            spacing: SPACING,
            radius: RADII,
            typography: TYPOGRAPHY,
        }
    }

    /// The default dark theme.
    pub fn dark() -> Self {
        Theme {
            bg: hex("#1c1c1e"),
            surface: hex("#2c2c2e"),
            fg: hex("#f5f5f7"),
            muted: hex("#98989d"),
            accent: hex("#0a84ff"),
            border: hex("#3a3a3c"),
            danger: hex("#ff453a"),
            spacing: SPACING,
            radius: RADII,
            typography: TYPOGRAPHY,
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::light()
    }
}
