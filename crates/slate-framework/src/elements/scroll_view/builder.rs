//! ScrollView constructor and builder methods.

use crate::element::{AnyElement, IntoElement};
use crate::reactive::Signal;
use crate::style::Style;

use super::ScrollView;

impl ScrollView {
    /// Create a new empty, clip-only ScrollView (no scroll offset wired yet).
    ///
    /// Add [`offset`](Self::offset) to make it scrollable; without it the
    /// container simply clips its children to its bounds.
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            layout_style: Style::default(),
            offset: None,
            offset_value: 0.0,
            scroll_speed: Self::DEFAULT_SCROLL_SPEED,
            last_id: None,
        }
    }

    /// Bind the caller-owned vertical scroll offset signal.
    ///
    /// The signal is read here (`get()`) — under the render observer during
    /// `View::render` — so the view re-renders and the content re-translates
    /// when the wheel/keyboard handlers call `set()`. The offset is in logical
    /// pixels, `0` = top; it is clamped to the scrollable range at paint.
    ///
    /// Create the signal once, outside `render`, so it survives the whole-view
    /// rebuild (e.g. store it on your `View` struct).
    pub fn offset(mut self, offset: Signal<f32>) -> Self {
        self.offset_value = offset.get();
        self.offset = Some(offset);
        self
    }

    /// Override the per-notch scroll distance for discrete wheels (logical px).
    /// Trackpad / precise-pixel deltas are unaffected.
    pub fn scroll_speed(mut self, speed: f32) -> Self {
        self.scroll_speed = speed.max(0.0);
        self
    }

    /// Configure the layout style via closure (sizing, padding, gap, direction).
    ///
    /// Give the ScrollView a fixed height (and/or width) so it forms a viewport
    /// smaller than its content — that overflow is what scrolls.
    pub fn style(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        self.layout_style = f(self.layout_style);
        self
    }

    /// Add a child element.
    pub fn child<E: IntoElement>(mut self, child: E) -> Self {
        self.children.push(AnyElement::new(child));
        self
    }

    /// Add a type-erased child element.
    pub fn child_any(mut self, child: AnyElement) -> Self {
        self.children.push(child);
        self
    }

    /// Add multiple children.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: IntoElement,
    {
        for child in children {
            self.children.push(AnyElement::new(child));
        }
        self
    }
}
