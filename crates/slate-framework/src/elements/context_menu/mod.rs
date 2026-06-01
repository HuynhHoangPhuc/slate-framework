//! ContextMenu — an in-canvas right-click menu wrapping a target.
//!
//! Wraps a target element; a right-click (secondary button) anywhere over it
//! opens an [`Overlay`] menu at the pointer. **In-canvas only** — the native OS
//! context menu is a separate, later concern; this is for canvas-drawn
//! affordances (a row, a chart point) that have no OS menu.
//!
//! Like [`Select`](crate::Select) the overlay is `modal(true).scrim(false)`:
//! focus moves into the menu on open (so arrow-key roving works), is trapped
//! while open, and is restored on close — all without dimming the canvas. Esc,
//! click-away, and choosing an item dismiss it.
//!
//! State is caller-owned: `open` (visibility) and `anchor` (the pointer rect the
//! menu opens at, written by the right-click handler). Create both outside
//! `render`.

use crate::element::{AnyElement, IntoElement, Sealed};
use crate::elements::menu::{MenuEntry, MenuList};
use crate::elements::{Div, Overlay, Placement};
use crate::reactive::Signal;
use crate::theme::theme;
use crate::types::Bounds;

use slate_platform::MouseButton;

/// Activation callback: receives the chosen item index.
type OnSelect = std::sync::Arc<dyn Fn(usize) + Send + Sync + 'static>;

/// An in-canvas right-click menu wrapping a target element.
///
/// # Example
/// ```ignore
/// ContextMenu::new(open.clone(), anchor.clone())
///     .items(["Rename", "Duplicate", "Delete"])
///     .on_select(move |i| run_action(i))
///     .child(row_content)
/// ```
pub struct ContextMenu {
    open: Signal<bool>,
    anchor: Signal<Bounds>,
    items: Vec<MenuEntry>,
    on_select: OnSelect,
    target: Option<AnyElement>,
}

impl ContextMenu {
    /// Create a context menu bound to caller-owned `open` + `anchor` signals.
    pub fn new(open: Signal<bool>, anchor: Signal<Bounds>) -> Self {
        Self {
            open,
            anchor,
            items: Vec::new(),
            on_select: std::sync::Arc::new(|_| {}),
            target: None,
        }
    }

    /// Set the menu items (any `&str` / `String` / [`MenuEntry`]).
    pub fn items<I, E>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<MenuEntry>,
    {
        self.items = items.into_iter().map(Into::into).collect();
        self
    }

    /// Set the activation callback (chosen item index).
    pub fn on_select<F: Fn(usize) + Send + Sync + 'static>(mut self, f: F) -> Self {
        self.on_select = std::sync::Arc::new(f);
        self
    }

    /// Set the wrapped target element that the right-click opens the menu over.
    pub fn child<E: IntoElement>(mut self, target: E) -> Self {
        self.target = Some(AnyElement::new(target));
        self
    }

    /// The pointer-anchored overlay menu.
    fn menu(&self) -> Overlay {
        let t = theme();
        let close = self.open.clone();
        let open = self.open.clone();
        let on_select = self.on_select.clone();
        let menu = MenuList::new(self.items.clone()).on_activate(move |i, _cx| {
            on_select(i);
            open.set(false);
        });
        Overlay::new()
            .anchor(self.anchor.clone())
            .placement(Placement::bottom())
            .gap(0.0)
            .modal(true)
            .scrim(false)
            .on_dismiss(move || close.set(false))
            .child(
                Div::new()
                    .background(t.surface)
                    .corner_radius(t.radius.md)
                    .border(t.border, 1.0)
                    .style(|s| s.column().padding_all(t.spacing.xs))
                    .child(menu),
            )
    }
}

impl Sealed for ContextMenu {}

impl IntoElement for ContextMenu {
    type Element = Div;

    fn into_element(self) -> Div {
        let open = self.open.get(); // subscribe so toggling rebuilds
        let anchor = self.anchor.clone();
        let set_open = self.open.clone();
        // Build the overlay before moving `self.target` out (it borrows `self`).
        let menu = open.then(|| self.menu());

        let mut root = Div::new().style(|s| s.column()).on_mouse_down(move |ev, _| {
            if ev.button == Some(MouseButton::Right) {
                let (x, y) = ev.position;
                // Zero-size rect at the pointer → menu opens just below it.
                anchor.set(Bounds::from_origin_size(x, y, 0.0, 0.0));
                set_open.set(true);
            }
        });
        if let Some(target) = self.target {
            root = root.child_any(target);
        }
        if let Some(menu) = menu {
            root = root.child(menu);
        }
        root
    }
}
