//! Div constructor and child-management builders.

use crate::element::{AnyElement, IntoElement};
use crate::style::Style;

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
