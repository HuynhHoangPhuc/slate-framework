//! Splitter — a two-pane resizable layout with a draggable divider.
//!
//! The only non-trivial layout container: it owns two pane slots plus a divider
//! hit region. The split is a caller-owned `Signal<f32>` ratio (the first pane's
//! fraction of the main axis), read under the render observer (Strategy A);
//! divider drag ([`handlers`]) and Arrow/Home/End keys write it. Proportional
//! sizing is imposed directly on the pane nodes via `flex_grow` (`ratio` /
//! `1 - ratio`) with `flex_basis: 0`, so panes re-derive their pixel sizes from
//! the ratio on every container resize; per-pane `min_size` clamps are enforced
//! by Taffy each layout pass. Ratio arithmetic + clamping lives in [`value`].
//!
//! The divider exposes `AccessibilityRole::Slider` with the current split
//! percentage and the Increment/Decrement actions a screen reader drives — it
//! is, semantically, a one-dimensional value control.

mod handlers;
mod layout;
mod value;

use crate::element::{AnyElement, IntoElement, Sealed};
use crate::reactive::Signal;
use crate::theme::theme;
use crate::types::{
    AccessibilityAction, AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, ElementId,
};

/// Divider thickness in logical pixels.
pub(super) const DIVIDER_THICK: f32 = 6.0;
/// Default per-pane minimum size in logical pixels.
const DEFAULT_MIN: f32 = 80.0;

/// Orientation of a [`Splitter`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitAxis {
    /// Panes side by side, divider runs vertically, drag along X.
    Horizontal,
    /// Panes stacked, divider runs horizontally, drag along Y.
    Vertical,
}

/// A two-pane resizable layout bound to a caller-owned split `Signal<f32>`
/// (the first pane's `0.0..=1.0` fraction of the main axis).
///
/// # Example
/// ```ignore
/// let split = Signal::new(cx.runtime(), 0.3);
/// Splitter::new(split)
///     .first(sidebar)
///     .second(main)
///     .min_sizes(160.0, 320.0)
/// ```
pub struct Splitter {
    ratio: Signal<f32>,
    /// `ratio` snapshot for this frame's layout + a11y.
    ratio_now: f32,
    first: Option<AnyElement>,
    second: Option<AnyElement>,
    axis: SplitAxis,
    min_first: f32,
    min_second: f32,
    label: String,
    /// Resting divider colour (theme `border`).
    divider: [f32; 4],
    last_id: Option<ElementId>,
    divider_id: Option<ElementId>,
}

/// Layout state for Splitter — the container node whose children are
/// `[first pane, divider, second pane]`.
pub struct SplitterLayout {
    container: taffy::NodeId,
}

impl Splitter {
    /// Create a horizontal splitter bound to `ratio` (first-pane fraction).
    pub fn new(ratio: Signal<f32>) -> Self {
        let t = theme();
        let ratio_now = ratio.get(); // subscribe the render observer (Strategy A)
        Self {
            ratio,
            ratio_now,
            first: None,
            second: None,
            axis: SplitAxis::Horizontal,
            min_first: DEFAULT_MIN,
            min_second: DEFAULT_MIN,
            label: "Split divider".to_string(),
            divider: t.border.into(),
            last_id: None,
            divider_id: None,
        }
    }

    /// Set the orientation (default [`SplitAxis::Horizontal`]).
    pub fn axis(mut self, axis: SplitAxis) -> Self {
        self.axis = axis;
        self
    }

    /// Shorthand for [`axis(SplitAxis::Vertical)`](Self::axis).
    pub fn vertical(mut self) -> Self {
        self.axis = SplitAxis::Vertical;
        self
    }

    /// Set the first (left/top) pane content.
    pub fn first<E: IntoElement>(mut self, pane: E) -> Self {
        self.first = Some(AnyElement::new(pane));
        self
    }

    /// Set the second (right/bottom) pane content.
    pub fn second<E: IntoElement>(mut self, pane: E) -> Self {
        self.second = Some(AnyElement::new(pane));
        self
    }

    /// Set the minimum size (logical px) of each pane; drag/keys clamp to these.
    pub fn min_sizes(mut self, first: f32, second: f32) -> Self {
        self.min_first = first.max(0.0);
        self.min_second = second.max(0.0);
        self
    }

    /// Set the divider's accessible name.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Main-axis length + origin of the given bounds for this orientation.
    fn main(&self, bounds: Bounds) -> (f32, f32) {
        match self.axis {
            SplitAxis::Horizontal => (bounds.size.width, bounds.origin.x),
            SplitAxis::Vertical => (bounds.size.height, bounds.origin.y),
        }
    }

    /// Build the leaf accessibility node for the divider: a Slider exposing the
    /// current split percentage and the Increment/Decrement actions.
    fn a11y_node(&self, id: ElementId, bounds: Bounds) -> AccessibilityNode {
        let pct = (self.ratio_now.clamp(0.0, 1.0) * 100.0).round() as i64;
        AccessibilityNode {
            id,
            bounds,
            info: AccessibilityInfo {
                role: AccessibilityRole::Slider,
                label: Some(self.label.clone()),
                value: Some(format!("{pct}%")),
                ..Default::default()
            },
            children: Vec::new(),
            actions: vec![
                AccessibilityAction::Focus,
                AccessibilityAction::Increment,
                AccessibilityAction::Decrement,
            ],
        }
    }
}

impl Sealed for Splitter {}

impl IntoElement for Splitter {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
