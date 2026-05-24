//! Div visual + layout-style builders.

use taffy::prelude::*;

use crate::color::Color;
use crate::reactive_value::Reactive;
use crate::style::Style;

use super::Div;

impl Div {
    /// Configure layout style via closure.
    ///
    /// # Example
    /// ```ignore
    /// Div::new().style(|s| s.padding_all(16.0).gap(8.0).column())
    /// ```
    pub fn style(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        self.layout_style = f(self.layout_style);
        self
    }

    /// Set background color (linear, premultiplied RGBA).
    ///
    /// Accepts a plain color or a signal-backed [`Reactive`]: a signal value
    /// is read here (under the render observer), so the view re-renders and the
    /// background re-resolves when that signal changes.
    pub fn background(mut self, color: impl Into<Reactive<Color>>) -> Self {
        let value: Reactive<Color> = color.into();
        self.visual.background = Some(value.resolve().0);
        self
    }

    /// Set corner radius in logical pixels.
    ///
    /// Accepts a plain `f32` or a signal-backed [`Reactive`]; see [`Self::background`].
    pub fn corner_radius(mut self, radius: impl Into<Reactive<f32>>) -> Self {
        let value: Reactive<f32> = radius.into();
        self.visual.corner_radius = value.resolve();
        self
    }

    /// Set flex direction (convenience method).
    pub fn direction(mut self, direction: FlexDirection) -> Self {
        self.layout_style.flex_direction = direction;
        self
    }

    /// Set flex direction to column (convenience method).
    pub fn column(mut self) -> Self {
        self.layout_style.flex_direction = FlexDirection::Column;
        self
    }

    /// Set flex direction to row (convenience method).
    pub fn row(mut self) -> Self {
        self.layout_style.flex_direction = FlexDirection::Row;
        self
    }

    /// Set flex grow (convenience method).
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.layout_style.flex_grow = grow;
        self
    }

    /// Set gap between children (convenience method).
    pub fn gap(mut self, gap: f32) -> Self {
        self.layout_style.gap = gap;
        self
    }
}
