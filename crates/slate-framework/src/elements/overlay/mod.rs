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
//! // `anchor` is a `Signal<Bounds>` the button writes via `Div::track_bounds`,
//! // so the popover follows the button's live rect (a fixed `Bounds` also
//! // works). The popover flips above the button if there's no room below.
//! Overlay::new()
//!     .anchor(anchor.clone())
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
mod registry;

pub use anchor::{Align, Placement, Side};
pub(crate) use registry::{ClickOutcome, OverlayEntry, OverlayRegistry};

use std::rc::Rc;

use crate::element::{AnyElement, IntoElement, Sealed};
use crate::style::Style;
use crate::types::{Bounds, ElementId, Point, Size};

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
    /// Modal mode: paints a full-viewport scrim behind the content and traps
    /// keyboard focus to the overlay subtree while open (focus is auto-moved in
    /// on open and restored on close). Default `false` (popover/tooltip).
    pub(super) modal: bool,
    /// Whether a modal overlay paints its dimming scrim. Default `true`. Set
    /// `false` to keep modal focus-management (auto-focus, trap, focus-restore,
    /// click-away dismiss) **without** the full-viewport dim — the right shape
    /// for a focus-managed popup menu (Select dropdown, in-canvas ContextMenu)
    /// that should not darken the app behind it. Ignored for non-modal overlays.
    pub(super) scrim: bool,
    /// Caller's dismiss callback, invoked when the user requests dismissal (Esc
    /// while top-most, click outside the content, or a modal scrim click). The
    /// overlay has no open/closed state of its own — the callback flips the
    /// caller's `open` signal so the overlay drops out of the next rebuild.
    /// `None` opts the overlay out of dismissal.
    pub(super) on_dismiss: Option<Rc<dyn Fn()>>,
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
/// px) resolved in prepaint and reused at paint so hit regions and pixels agree,
/// plus the viewport size captured at prepaint for the modal scrim rect.
pub struct OverlayPaintState {
    pub(super) solved_origin: Point,
    pub(super) viewport: Size,
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
