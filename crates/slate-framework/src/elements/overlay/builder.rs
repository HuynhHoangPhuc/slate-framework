//! Overlay constructor and builder methods.

use crate::element::{AnyElement, IntoElement};
use crate::style::Style;
use crate::types::Bounds;

use super::{Overlay, Placement};

impl Overlay {
    /// Create a new empty overlay anchored at the origin, placed below-center.
    ///
    /// Set a real [`anchor`](Self::anchor) and [`child`](Self::child) before
    /// use; the defaults render nothing useful on their own.
    pub fn new() -> Self {
        Self {
            content: None,
            anchor: Bounds::ZERO,
            placement: Placement::default(),
            depth: Self::DEFAULT_DEPTH,
            gap: Self::DEFAULT_GAP,
            layout_style: Style::default(),
            last_id: None,
        }
    }

    /// Set the absolute (window-relative) target rect the content anchors to.
    pub fn anchor(mut self, anchor: Bounds) -> Self {
        self.anchor = anchor;
        self
    }

    /// Set the preferred placement (side + cross-axis alignment). The solver
    /// still flips/shifts at viewport edges.
    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    /// Set the overlay's z-depth. Higher draws on top; the base tree is depth 0.
    /// Use distinct ascending depths to stack overlays (tooltip < dropdown <
    /// dialog).
    pub fn depth(mut self, depth: i32) -> Self {
        self.depth = depth;
        self
    }

    /// Set the main-axis gap (logical px) between the anchor edge and content.
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    /// Set the content child painted into the overlay layer.
    ///
    /// Visual styling (background, padding, corner radius) belongs on this
    /// child — the overlay node itself paints nothing.
    pub fn child<E: IntoElement>(mut self, child: E) -> Self {
        self.content = Some(AnyElement::new(child));
        self
    }

    /// Apply sizing constraints to the overlay node (e.g. `max_width`).
    ///
    /// This is **not** for visual styling — the overlay node is a transparent
    /// positioning wrapper. The resulting measured size feeds the anchor solver.
    pub fn style<F: FnOnce(Style) -> Style>(mut self, f: F) -> Self {
        self.layout_style = f(self.layout_style);
        self
    }
}
