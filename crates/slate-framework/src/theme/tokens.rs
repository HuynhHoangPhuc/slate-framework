//! Semantic design-token taxonomy.
//!
//! A [`Theme`] is a flat bag of *semantic* tokens — colours named by role
//! (`bg`, `surface`, `accent`, …) rather than by hue, plus spacing / radius /
//! type scales. Widgets read tokens through the ambient accessor
//! ([`crate::theme::theme`]) so they never hard-code raw values.
//!
//! All token types are `#[non_exhaustive]`: adopters build a custom theme by
//! cloning [`Theme::light`]/[`Theme::dark`] and tweaking fields, so **adding a
//! new token in a later release is additive, not a breaking change**.

use crate::color::Color;

/// Spacing scale in logical pixels.
///
/// A small, fixed t-shirt scale. Use these for padding, margins, and gaps so
/// spacing stays consistent across widgets.
#[derive(Copy, Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Spacing {
    /// Extra-small gap (tight inline spacing).
    pub xs: f32,
    /// Small gap.
    pub sm: f32,
    /// Medium gap (default control padding).
    pub md: f32,
    /// Large gap (section padding).
    pub lg: f32,
    /// Extra-large gap (page-level spacing).
    pub xl: f32,
}

/// Corner-radius scale in logical pixels.
#[derive(Copy, Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Radii {
    /// Small radius (chips, inputs).
    pub sm: f32,
    /// Medium radius (cards, buttons).
    pub md: f32,
    /// Large radius (panels, dialogs).
    pub lg: f32,
}

/// A single text style: size, weight, and line-height multiplier.
#[derive(Copy, Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct TypeStyle {
    /// Font size in logical pixels.
    pub size: f32,
    /// Font weight (CSS-style numeric, e.g. `400` regular, `600` semibold).
    pub weight: f32,
    /// Line-height as a multiple of `size` (e.g. `1.4`).
    pub line_height: f32,
}

/// Typographic scale of named roles.
#[derive(Copy, Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Typography {
    /// Small secondary text (captions, hints).
    pub caption: TypeStyle,
    /// Default body text.
    pub body: TypeStyle,
    /// Section / dialog heading.
    pub heading: TypeStyle,
}

/// A complete set of semantic design tokens.
///
/// Read ambiently inside `View::render` via [`crate::theme::theme`]; switch
/// light/dark by flipping the app's [`ThemeMode`](crate::theme::ThemeMode)
/// signal. `Copy` so resolving the active theme each frame is cheap.
#[derive(Copy, Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Theme {
    /// Window / app background.
    pub bg: Color,
    /// Raised surface (cards, panels) sitting above `bg`.
    pub surface: Color,
    /// Primary foreground / text colour.
    pub fg: Color,
    /// De-emphasised foreground (secondary text, disabled).
    pub muted: Color,
    /// Accent / primary-action colour.
    pub accent: Color,
    /// Hairline border / divider colour.
    pub border: Color,
    /// Destructive / error colour.
    pub danger: Color,
    /// Spacing scale.
    pub spacing: Spacing,
    /// Corner-radius scale.
    pub radius: Radii,
    /// Typographic scale.
    pub typography: Typography,
}

/// Shared spacing scale (mode-independent).
pub(super) const SPACING: Spacing = Spacing {
    xs: 4.0,
    sm: 8.0,
    md: 12.0,
    lg: 16.0,
    xl: 24.0,
};

/// Shared radius scale (mode-independent).
pub(super) const RADII: Radii = Radii {
    sm: 4.0,
    md: 8.0,
    lg: 12.0,
};

/// Shared typographic scale (mode-independent).
pub(super) const TYPOGRAPHY: Typography = Typography {
    caption: TypeStyle {
        size: 12.0,
        weight: 400.0,
        line_height: 1.3,
    },
    body: TypeStyle {
        size: 14.0,
        weight: 400.0,
        line_height: 1.4,
    },
    heading: TypeStyle {
        size: 20.0,
        weight: 600.0,
        line_height: 1.25,
    },
};
