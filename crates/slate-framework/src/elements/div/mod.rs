//! Div — flexbox container element.
//!
//! Div is the primary container element, supporting:
//! - Flexbox layout via Taffy (using unified Style type)
//! - Background color with corner radius
//! - Padding and margin
//! - Child elements

mod builder;
mod handlers;
mod layout;
mod style;

use crate::element::{AnyElement, IntoElement, Sealed};
use crate::event::{
    ElementKeyHandler, ElementTextInputHandler, MouseHandler, PointerHandler, ScrollHandler,
};
use crate::interaction_state::StateStyles;
use crate::reactive::Signal;
use crate::style::Style;
use crate::types::{Bounds, ElementId};

/// Flexbox container element.
///
/// # Example
///
/// ```ignore
/// Div::new()
///     .background([0.2, 0.2, 0.2, 1.0])
///     .corner_radius(8.0)
///     .style(|s| s.padding_all(16.0).gap(8.0))
///     .child(Text::new("Hello"))
///     .child(Text::new("World"))
/// ```
pub struct Div {
    pub(super) children: Vec<AnyElement>,
    pub(super) layout_style: Style,
    pub(super) visual: DivVisual,
    /// Per-interaction-state visual overrides (hover/active/disabled).
    /// `None` keeps the zero-cost fast path: no allocation, no state lookup
    /// at paint, byte-identical to a plain styled `Div`.
    pub(super) state_styles: Option<Box<StateStyles>>,
    /// When true the element is non-interactive: handlers/focus/IME are not
    /// registered during prepaint, the disabled style override wins at paint,
    /// and `is_disabled` is reported to AccessKit.
    pub(super) disabled: bool,
    /// User-provided stability key for dynamic lists (consumed during prepaint).
    pub(super) user_key: Option<String>,
    /// Stable ElementId allocated during prepaint (available after prepaint).
    pub(super) last_id: Option<ElementId>,
    /// Caller-owned signal that receives this element's painted bounds each
    /// frame (absolute, window-relative). Lets an [`Overlay`](crate::Overlay)
    /// anchor to this element's live rect (anchor-to-element). The write is
    /// guarded (only on change) so it never spins the whole-view rebuild loop.
    pub(super) track_bounds: Option<Signal<Bounds>>,
    /// Optional accessible name for this Div's `Group` node. `None` leaves the
    /// group unlabelled (the default). Container widgets (Panel) set it so a
    /// screen reader announces the group by its title.
    pub(super) a11y_label: Option<String>,
    /// Optional role override for this Div's a11y node. `None` => the default
    /// `Group`. A styled composite (e.g. a Select trigger built from a Div) sets
    /// this so the node announces with a meaningful role instead of `Group`.
    pub(super) a11y_role: Option<crate::types::AccessibilityRole>,
    // -------------------------------------------------------------------------
    // Event handlers
    // -------------------------------------------------------------------------
    /// Handler for synthesized click events (down+up on same target).
    pub(crate) on_click: Option<MouseHandler>,
    /// Handler for mouse button down events.
    pub(crate) on_mouse_down: Option<MouseHandler>,
    /// Handler for mouse button up events.
    pub(crate) on_mouse_up: Option<MouseHandler>,
    /// Handler for mouse move events (coalesced, one per frame).
    pub(crate) on_mouse_move: Option<MouseHandler>,
    /// Handler for scroll wheel/trackpad events.
    pub(crate) on_mouse_scrolled: Option<ScrollHandler>,
    /// Handler for raw pointer events (no coalescing).
    pub(crate) on_pointer_event: Option<PointerHandler>,
    /// Handler for pointer enter events.
    pub(crate) on_pointer_enter: Option<PointerHandler>,
    /// Handler for pointer leave events.
    pub(crate) on_pointer_leave: Option<PointerHandler>,
    // -------------------------------------------------------------------------
    // Focus configuration
    // -------------------------------------------------------------------------
    /// True if this element opts in to keyboard focus (registers with `FocusRegistry`).
    pub(crate) focusable: bool,
    /// W3C-style tab index. Negative excludes from Tab cycle but still allows
    /// programmatic focus via `AppContext::set_focus`.
    pub(crate) tab_index: i32,
    /// Whether the framework-provided focus ring is painted when focused.
    pub(crate) focus_ring: bool,
    // -------------------------------------------------------------------------
    // Per-element keyboard handlers
    // -------------------------------------------------------------------------
    /// Handler for KeyDown events while this element is on the focused chain.
    pub(crate) on_key_down: Option<ElementKeyHandler>,
    /// Handler for KeyUp events while this element is on the focused chain.
    pub(crate) on_key_up: Option<ElementKeyHandler>,
    /// Handler for composed text input while this element is on the focused chain.
    pub(crate) on_text_input: Option<ElementTextInputHandler>,
    // -------------------------------------------------------------------------
    // IME configuration + handlers
    // -------------------------------------------------------------------------
    /// True if this element opts in to IME composition. The framework
    /// registers an `ImeState` entry each prepaint when this is set; OS
    /// sync queries answered via the published `CachedImeQuery` snapshot.
    pub(crate) ime_capable: bool,
    /// Handler for IME preedit updates while this element is on the focused
    /// chain.
    pub(crate) on_ime_preedit: Option<crate::event::ElementImePreeditHandler>,
    /// Handler for IME commits while this element is on the focused chain.
    /// Empty commit text is the "clear preedit, no insert" signal.
    pub(crate) on_ime_commit: Option<crate::event::ElementImeCommitHandler>,
}

/// A hairline border for a [`Div`], drawn as an outer frame behind the
/// background, which is inset by [`width`](Border::width) on every edge.
///
/// There is no stroke pipeline in the renderer (only filled rects), so a border
/// is two filled rounded rects: the outer frame in the border colour, then the
/// background painted inset on top. Cheap and KISS, like the 4-rect focus ring.
#[derive(Clone, Copy, Debug)]
pub struct Border {
    /// Border colour (linear, premultiplied RGBA).
    pub color: [f32; 4],
    /// Border thickness in logical pixels (inset applied to the background).
    pub width: f32,
}

/// Visual styling for a Div (non-layout properties).
#[derive(Clone, Debug, Default)]
pub struct DivVisual {
    /// Background color (linear, premultiplied RGBA).
    pub background: Option<[f32; 4]>,
    /// Corner radius in logical pixels.
    pub corner_radius: f32,
    /// Optional hairline border drawn behind the (inset) background.
    pub border: Option<Border>,
}

/// Layout state for Div — stores Taffy node ID.
pub struct DivLayoutState {
    pub(super) node_id: taffy::NodeId,
}

/// Paint state for Div — currently empty.
pub struct DivPaintState;

impl Default for Div {
    fn default() -> Self {
        Self::new()
    }
}

impl Sealed for Div {}

impl IntoElement for Div {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
