//! Div constructor and child-management builders.

use crate::element::{AnyElement, IntoElement};
use crate::reactive::Signal;
use crate::style::Style;
use crate::types::Bounds;

use super::{Div, DivVisual};

impl Div {
    /// Create a new empty Div.
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            layout_style: Style::default(),
            visual: DivVisual::default(),
            state_styles: None,
            disabled: false,
            user_key: None,
            last_id: None,
            track_bounds: None,
            on_click: None,
            on_mouse_down: None,
            on_mouse_up: None,
            on_mouse_move: None,
            on_mouse_scrolled: None,
            on_pointer_event: None,
            on_pointer_enter: None,
            on_pointer_leave: None,
            focusable: false,
            tab_index: 0,
            focus_ring: true,
            on_key_down: None,
            on_key_up: None,
            on_text_input: None,
            ime_capable: false,
            on_ime_preedit: None,
            on_ime_commit: None,
        }
    }

    /// Set a stability key for dynamic lists.
    ///
    /// Use when child order or count changes between frames (e.g., list items).
    /// Static trees can omit — tree-position keying handles them automatically.
    pub fn key(mut self, k: impl Into<String>) -> Self {
        self.user_key = Some(k.into());
        self
    }

    /// Report this element's painted bounds into a caller-owned signal each
    /// frame, enabling **anchor-to-element** overlays.
    ///
    /// The signal receives the element's absolute (window-relative) [`Bounds`]
    /// once known during prepaint. Pass the same signal to
    /// [`Overlay::anchor`](crate::Overlay::anchor) so the overlay tracks this
    /// element's live rect (e.g. a popover anchored under a button that
    /// re-positions on window resize) instead of a hardcoded rect.
    ///
    /// The write is guarded — it fires only when the bounds change — so it does
    /// not spin the Strategy-A whole-view rebuild loop. Create `bounds` once
    /// outside `render` (like a scroll offset) so it survives view rebuilds.
    ///
    /// The signal reflects the element's *previous-prepaint* rect, so opening an
    /// overlay the same frame the trigger first mounts shows one frame at the
    /// signal's initial value before it snaps into place. Keeping a popover
    /// closed until its trigger has painted once (the common case) avoids this.
    pub fn track_bounds(mut self, bounds: Signal<Bounds>) -> Self {
        self.track_bounds = Some(bounds);
        self
    }

    /// Add a child element.
    pub fn child<E: IntoElement>(mut self, child: E) -> Self {
        self.children.push(AnyElement::new(child));
        self
    }

    /// Add a type-erased child element.
    ///
    /// Use this when you already have an `AnyElement` (e.g., from a builder).
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
