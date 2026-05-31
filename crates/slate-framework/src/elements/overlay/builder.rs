//! Overlay constructor and builder methods.

use crate::element::{AnyElement, IntoElement};
use crate::reactive_value::Reactive;
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
            modal: false,
            layout_style: Style::default(),
            last_id: None,
        }
    }

    /// Set the target rect the content anchors to.
    ///
    /// Accepts a fixed [`Bounds`] or a signal-backed [`Reactive`] for
    /// **anchor-to-element**: pair a `Signal<Bounds>` here with
    /// [`Div::track_bounds`](crate::Div::track_bounds) on the target so the
    /// overlay follows that element's live painted rect (e.g. a popover under a
    /// button that re-anchors on window resize) instead of a hardcoded rect. A
    /// signal value is read here under the render observer, so the view
    /// re-renders and the overlay re-anchors when the tracked bounds change.
    /// The resolved rect is the tracked element's *previous-prepaint* bounds
    /// (see [`Div::track_bounds`](crate::Div::track_bounds)); a change re-anchors
    /// on the next frame.
    pub fn anchor(mut self, anchor: impl Into<Reactive<Bounds>>) -> Self {
        let value: Reactive<Bounds> = anchor.into();
        self.anchor = value.resolve();
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

    /// Make the overlay **modal**: paint a full-viewport scrim behind the
    /// content and trap keyboard focus to the overlay subtree while open.
    ///
    /// On open, focus moves to the overlay's first focusable; Tab/Shift+Tab
    /// cycle only within the overlay; on close, the previously-focused element
    /// is restored. Use for dialogs. Non-modal (the default) is for
    /// popovers/tooltips/dropdowns that leave focus and the base tree alone.
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
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
