//! Style types for layout configuration.
//!
//! Provides a framework-level `Style` type that abstracts over Taffy's
//! layout properties. This isolation layer:
//! - Simplifies the API for framework users
//! - Insulates against Taffy API changes
//! - Provides sensible defaults for UI development

use taffy::prelude::*;

use crate::types::Edges;

/// Length value for layout properties.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub enum Length {
    /// Size determined by content or parent.
    #[default]
    Auto,
    /// Fixed size in logical pixels.
    Px(f32),
    /// Percentage of parent size.
    Percent(f32),
}

impl Length {
    /// Create a pixel length.
    pub const fn px(value: f32) -> Self {
        Self::Px(value)
    }

    /// Create a percentage length.
    pub const fn percent(value: f32) -> Self {
        Self::Percent(value)
    }
}

/// Display mode for an element.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum DisplayMode {
    /// Flexbox layout (default).
    #[default]
    Flex,
    /// CSS Grid layout.
    Grid,
    /// Element is hidden and takes no space.
    None,
}

/// Position mode for an element.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Position {
    /// Normal flow positioning (default).
    #[default]
    Relative,
    /// Positioned relative to nearest positioned ancestor.
    Absolute,
}

/// Overflow handling.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Overflow {
    /// Content is visible outside bounds.
    #[default]
    Visible,
    /// Content is clipped to bounds.
    Hidden,
    /// Scroll if content overflows.
    Scroll,
}

/// Size constraint with min/max.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct SizeConstraint {
    pub width: Length,
    pub height: Length,
}

impl SizeConstraint {
    pub const AUTO: Self = Self {
        width: Length::Auto,
        height: Length::Auto,
    };

    pub const fn new(width: Length, height: Length) -> Self {
        Self { width, height }
    }

    pub const fn px(width: f32, height: f32) -> Self {
        Self {
            width: Length::Px(width),
            height: Length::Px(height),
        }
    }
}

/// Layout style for elements.
///
/// Covers common Flexbox properties with sensible defaults.
/// Grid properties to be added in future versions.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    // Display
    pub display: DisplayMode,

    // Flexbox
    pub flex_direction: FlexDirection,
    pub align_items: AlignItems,
    pub justify_content: JustifyContent,
    pub flex_wrap: FlexWrap,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Length,

    // Spacing
    pub padding: Edges<Length>,
    pub margin: Edges<Length>,
    pub gap: f32,

    // Sizing
    pub size: SizeConstraint,
    pub min_size: SizeConstraint,
    pub max_size: SizeConstraint,

    // Positioning
    pub position: Position,
    pub overflow: Overflow,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            display: DisplayMode::Flex,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Stretch,
            justify_content: JustifyContent::FlexStart,
            flex_wrap: FlexWrap::NoWrap,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            padding: Edges::default(),
            margin: Edges::default(),
            gap: 0.0,
            size: SizeConstraint::AUTO,
            min_size: SizeConstraint::AUTO,
            max_size: SizeConstraint::AUTO,
            position: Position::Relative,
            overflow: Overflow::Visible,
        }
    }
}

impl Style {
    /// Create a new style with defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set display mode.
    pub fn display(mut self, display: DisplayMode) -> Self {
        self.display = display;
        self
    }

    /// Set flex direction.
    pub fn flex_direction(mut self, direction: FlexDirection) -> Self {
        self.flex_direction = direction;
        self
    }

    /// Set flex direction to column.
    pub fn column(mut self) -> Self {
        self.flex_direction = FlexDirection::Column;
        self
    }

    /// Set flex direction to row.
    pub fn row(mut self) -> Self {
        self.flex_direction = FlexDirection::Row;
        self
    }

    /// Set align items.
    pub fn align_items(mut self, align: AlignItems) -> Self {
        self.align_items = align;
        self
    }

    /// Set justify content.
    pub fn justify_content(mut self, justify: JustifyContent) -> Self {
        self.justify_content = justify;
        self
    }

    /// Set flex grow.
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.flex_grow = grow;
        self
    }

    /// Set flex shrink.
    pub fn flex_shrink(mut self, shrink: f32) -> Self {
        self.flex_shrink = shrink;
        self
    }

    /// Set uniform padding in logical pixels.
    pub fn padding_all(mut self, value: f32) -> Self {
        self.padding = Edges::all(Length::Px(value));
        self
    }

    /// Set padding with edges.
    pub fn padding(mut self, edges: Edges<Length>) -> Self {
        self.padding = edges;
        self
    }

    /// Set uniform margin in logical pixels.
    pub fn margin_all(mut self, value: f32) -> Self {
        self.margin = Edges::all(Length::Px(value));
        self
    }

    /// Set margin with edges.
    pub fn margin(mut self, edges: Edges<Length>) -> Self {
        self.margin = edges;
        self
    }

    /// Set gap between children.
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    /// Set fixed width in logical pixels.
    pub fn width(mut self, width: f32) -> Self {
        self.size.width = Length::Px(width);
        self
    }

    /// Set fixed height in logical pixels.
    pub fn height(mut self, height: f32) -> Self {
        self.size.height = Length::Px(height);
        self
    }

    /// Set size constraint.
    pub fn size(mut self, size: SizeConstraint) -> Self {
        self.size = size;
        self
    }

    /// Set min size constraint.
    pub fn min_size(mut self, size: SizeConstraint) -> Self {
        self.min_size = size;
        self
    }

    /// Set max size constraint.
    pub fn max_size(mut self, size: SizeConstraint) -> Self {
        self.max_size = size;
        self
    }

    /// Set position mode.
    pub fn position(mut self, position: Position) -> Self {
        self.position = position;
        self
    }

    /// Set overflow handling.
    pub fn overflow(mut self, overflow: Overflow) -> Self {
        self.overflow = overflow;
        self
    }
}

// Conversion: Length -> taffy::Dimension
fn length_to_dimension(len: Length) -> Dimension {
    match len {
        Length::Auto => Dimension::auto(),
        Length::Px(v) => Dimension::length(v),
        Length::Percent(v) => Dimension::percent(v / 100.0),
    }
}

// Conversion: Length -> taffy::LengthPercentage
fn length_to_length_percentage(len: Length) -> LengthPercentage {
    match len {
        Length::Auto => LengthPercentage::length(0.0),
        Length::Px(v) => LengthPercentage::length(v),
        Length::Percent(v) => LengthPercentage::percent(v / 100.0),
    }
}

// Conversion: Length -> taffy::LengthPercentageAuto
fn length_to_length_percentage_auto(len: Length) -> LengthPercentageAuto {
    match len {
        Length::Auto => LengthPercentageAuto::auto(),
        Length::Px(v) => LengthPercentageAuto::length(v),
        Length::Percent(v) => LengthPercentageAuto::percent(v / 100.0),
    }
}

impl From<&Style> for taffy::Style {
    fn from(s: &Style) -> taffy::Style {
        taffy::Style {
            display: match s.display {
                DisplayMode::Flex => Display::Flex,
                DisplayMode::Grid => Display::Grid,
                DisplayMode::None => Display::None,
            },
            flex_direction: s.flex_direction,
            align_items: Some(s.align_items),
            justify_content: Some(s.justify_content),
            flex_wrap: s.flex_wrap,
            flex_grow: s.flex_grow,
            flex_shrink: s.flex_shrink,
            flex_basis: length_to_dimension(s.flex_basis),
            padding: Rect {
                left: length_to_length_percentage(s.padding.left),
                right: length_to_length_percentage(s.padding.right),
                top: length_to_length_percentage(s.padding.top),
                bottom: length_to_length_percentage(s.padding.bottom),
            },
            margin: Rect {
                left: length_to_length_percentage_auto(s.margin.left),
                right: length_to_length_percentage_auto(s.margin.right),
                top: length_to_length_percentage_auto(s.margin.top),
                bottom: length_to_length_percentage_auto(s.margin.bottom),
            },
            gap: taffy::Size {
                width: LengthPercentage::length(s.gap),
                height: LengthPercentage::length(s.gap),
            },
            size: taffy::Size {
                width: length_to_dimension(s.size.width),
                height: length_to_dimension(s.size.height),
            },
            min_size: taffy::Size {
                width: length_to_dimension(s.min_size.width),
                height: length_to_dimension(s.min_size.height),
            },
            max_size: taffy::Size {
                width: length_to_dimension(s.max_size.width),
                height: length_to_dimension(s.max_size.height),
            },
            position: match s.position {
                Position::Relative => taffy::Position::Relative,
                Position::Absolute => taffy::Position::Absolute,
            },
            overflow: taffy::Point {
                x: match s.overflow {
                    Overflow::Visible => taffy::Overflow::Visible,
                    Overflow::Hidden => taffy::Overflow::Clip,
                    Overflow::Scroll => taffy::Overflow::Scroll,
                },
                y: match s.overflow {
                    Overflow::Visible => taffy::Overflow::Visible,
                    Overflow::Hidden => taffy::Overflow::Clip,
                    Overflow::Scroll => taffy::Overflow::Scroll,
                },
            },
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_default() {
        let s = Style::default();
        assert_eq!(s.display, DisplayMode::Flex);
        assert_eq!(s.flex_direction, FlexDirection::Row);
        assert_eq!(s.flex_grow, 0.0);
        assert_eq!(s.flex_shrink, 1.0);
    }

    #[test]
    fn style_builder() {
        let s = Style::new()
            .column()
            .padding_all(16.0)
            .gap(8.0)
            .flex_grow(1.0);

        assert_eq!(s.flex_direction, FlexDirection::Column);
        assert_eq!(s.padding, Edges::all(Length::Px(16.0)));
        assert_eq!(s.gap, 8.0);
        assert_eq!(s.flex_grow, 1.0);
    }

    #[test]
    fn style_to_taffy() {
        let s = Style::new().padding_all(10.0).width(100.0).height(50.0);
        let taffy_style: taffy::Style = (&s).into();

        assert_eq!(taffy_style.display, Display::Flex);
        // Check padding conversion
        assert_eq!(taffy_style.padding.left, LengthPercentage::length(10.0));
    }
}
