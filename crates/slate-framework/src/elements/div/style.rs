//! Div visual + layout-style builders.

use taffy::prelude::*;

use crate::color::Color;
use crate::interaction_state::{StateStyle, StateStyles};
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

    /// Draw a hairline [`Border`](super::Border) of `color` and `width` logical
    /// pixels: an outer frame in `color` with the background inset on top. A
    /// `width <= 0.0` clears the border.
    pub fn border(mut self, color: impl Into<Color>, width: f32) -> Self {
        self.visual.border = (width > 0.0).then(|| super::Border {
            color: color.into().into(),
            width,
        });
        self
    }

    /// Set the accessible name announced for this Div's `Group` node.
    ///
    /// Used by container widgets so a screen reader names the group by its
    /// title; leaving it unset keeps the group unlabelled.
    pub fn a11y_label(mut self, label: impl Into<String>) -> Self {
        self.a11y_label = Some(label.into());
        self
    }

    /// Style overrides applied while the pointer is over this element (or a
    /// descendant). Falls back to the base style for any field left unset.
    ///
    /// # Example
    /// ```ignore
    /// Div::new().background(Color::WHITE).hover(|s| s.background(Color::BLUE))
    /// ```
    pub fn hover(mut self, f: impl FnOnce(StateStyle) -> StateStyle) -> Self {
        let styles = self.state_styles.get_or_insert_with(|| Box::new(StateStyles::default()));
        styles.hover = f(std::mem::take(&mut styles.hover));
        self
    }

    /// Style overrides applied while a primary-button press is active on this
    /// element (the "pressed"/`:active` state).
    pub fn active(mut self, f: impl FnOnce(StateStyle) -> StateStyle) -> Self {
        let styles = self.state_styles.get_or_insert_with(|| Box::new(StateStyles::default()));
        styles.active = f(std::mem::take(&mut styles.active));
        self
    }

    /// Style overrides applied while the element is [disabled](Self::disabled).
    /// Disabled wins over hover/active when resolving the painted style.
    pub fn disabled_style(mut self, f: impl FnOnce(StateStyle) -> StateStyle) -> Self {
        let styles = self.state_styles.get_or_insert_with(|| Box::new(StateStyles::default()));
        styles.disabled = f(std::mem::take(&mut styles.disabled));
        self
    }

    /// Mark this element disabled. A disabled element registers no event,
    /// focus, or IME handlers (so it ignores clicks, hover callbacks, and Tab),
    /// paints its [`disabled_style`](Self::disabled_style) override, and reports
    /// `is_disabled` to AccessKit.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
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
