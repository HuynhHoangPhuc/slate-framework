//! Overlay — paints its content **above the normal tree**, anchored to a rect.
//!
//! An Overlay is the foundation for popovers, tooltips, dropdowns and dialogs.
//! It carries a single content child but, unlike a normal container, the child
//! is painted into a **high-depth, unclipped scene layer** so it draws on top of
//! everything below it and escapes any enclosing scroll/clip — even when the
//! Overlay is declared deep inside a clipped subtree. Ordering goes through the
//! production depth-sort (`DepthBucketOrder`), not emission order.
//!
//! # Positioning
//!
//! The Overlay's own layout node is forced to [`Position::Absolute`](crate::style::Position::Absolute)
//! so it is removed from sibling flow (declaring an overlay never shifts the
//! widgets around it). Its painted position is **not** the layout location —
//! it's computed by the pure [`anchor`] solver from a caller-supplied anchor
//! rect, a [`Placement`], and the viewport, with flip/shift at window edges.
//!
//! ```ignore
//! // `button_bounds` is the target's absolute rect (e.g. captured from a hit
//! // region). The popover flips above the button if there's no room below.
//! Overlay::new()
//!     .anchor(button_bounds)
//!     .placement(Placement::bottom())
//!     .child(Div::new().background(...).child(Text::new("Menu")))
//! ```
//!
//! # Visual styling lives on the content
//!
//! The Overlay node is a bare positioning wrapper that paints nothing itself;
//! background, padding, corner radius, etc. belong on the content child. The
//! Overlay's own [`style`](Overlay::style) is for sizing constraints only
//! (e.g. `max_width` for a long menu), which the anchor solver reads back as the
//! measured content size.
//!
//! # Known limitation: clipped content does not yet inherit the overlay depth
//!
//! Overlay content that is itself **clipped** (e.g. a `ScrollView` body) is
//! painted by the renderer clip path, which opens its layer at depth `0`. So
//! the clipped portion sorts with the base tree rather than at the overlay
//! depth — a scrollable popover body would not draw on top. Unclipped content
//! (the common popover/tooltip/dropdown case) is unaffected. Lifting clipped
//! content to the overlay depth needs the renderer clip path to carry a depth;
//! that lands with scrollable-body support, not in this foundation slice.

mod anchor;
mod builder;
mod layout;

pub use anchor::{Align, Placement, Side};

use crate::element::{AnyElement, IntoElement, Sealed};
use crate::style::Style;
use crate::types::{Bounds, ElementId, Point};

/// An element whose content paints above the tree, anchored to a target rect.
///
/// See the [module docs](self) for the positioning and styling model.
pub struct Overlay {
    /// The single content child painted into the overlay layer. `None` renders
    /// nothing (the overlay still occupies no flow space).
    pub(super) content: Option<AnyElement>,
    /// Absolute (window-relative) target rect the content anchors to.
    pub(super) anchor: Bounds,
    /// Preferred side + cross-axis alignment; flips/shifts at viewport edges.
    pub(super) placement: Placement,
    /// Z-depth of the overlay's scene layer. Higher draws on top; the base tree
    /// is depth 0. Stacked overlays order by ascending depth.
    pub(super) depth: i32,
    /// Main-axis gap (logical px) between the anchor edge and the content.
    pub(super) gap: f32,
    /// Sizing constraints for the overlay node (NOT visual styling). Forced to
    /// `Position::Absolute` at layout so the overlay leaves sibling flow.
    pub(super) layout_style: Style,
    /// Stable ElementId allocated during prepaint (available after prepaint).
    pub(super) last_id: Option<ElementId>,
}

/// Layout state for Overlay — the Taffy node ID of the overlay wrapper.
pub struct OverlayLayoutState {
    pub(super) node_id: taffy::NodeId,
}

/// Paint state for Overlay — the anchor-solved content origin (absolute logical
/// px) resolved in prepaint and reused at paint so hit regions and pixels agree.
pub struct OverlayPaintState {
    pub(super) solved_origin: Point,
}

impl Overlay {
    /// Default z-depth for overlays. Well above the base tree (depth 0) with
    /// headroom below it for future framework chrome that must sit on top.
    pub(super) const DEFAULT_DEPTH: i32 = 1000;

    /// Default main-axis gap between the anchor and the content (logical px).
    pub(super) const DEFAULT_GAP: f32 = 4.0;
}

impl Default for Overlay {
    fn default() -> Self {
        Self::new()
    }
}

impl Sealed for Overlay {}

impl IntoElement for Overlay {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
