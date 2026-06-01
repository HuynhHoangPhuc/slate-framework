//! MenuList — a keyboard-navigable vertical list of selectable items.
//!
//! `MenuList` is the shared core of the overlay-based menu widgets
//! ([`Select`](crate::Select) and [`ContextMenu`](crate::ContextMenu)): a column
//! of labelled rows with a **roving-focus** keyboard model (the list container is
//! the tab stop; arrow keys move real focus to the neighbouring row so a screen
//! reader follows along — the pattern validated by the S0 DataGrid spike, here
//! reduced to one dimension). It emits a `Menu` → `MenuItem` accessibility
//! subtree and activates a row on click or Enter.
//!
//! ## State is self-owned (transient)
//!
//! The roving "active" row index lives in a per-element
//! [`StateRegistry`](crate::reactive_state::StateRegistry) slot
//! ([`use_state`]) keyed by the list's id, initialised to
//! [`initial_active`](MenuList::initial_active). It is transient UI state, not
//! caller data — when the menu unmounts (its overlay closes) the slot is
//! garbage-collected, so the next open starts fresh from `initial_active` again.
//! The meaningful result is delivered through the
//! [`on_activate`](MenuList::on_activate) callback, which the embedding widget
//! wires to its own caller-owned signals.
//!
//! [`use_state`]: crate::reactive_state::StateRegistry::use_state

mod keys;
mod layout;
mod prepaint;

use std::sync::Arc;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{AnyElement, Element, IntoElement, Sealed};
use crate::event::EventCtx;
use crate::theme::theme;
use crate::types::{Bounds, ElementId, LayoutId};

use super::control_paint::presentational_label;

/// Activation callback shared by click + Enter. Receives the row index and the
/// [`EventCtx`] so the embedding widget can update its own signals / run an
/// action and stop propagation.
pub(crate) type MenuActivate = Arc<dyn Fn(usize, &mut EventCtx) + Send + Sync + 'static>;

/// One row of a [`MenuList`]: a visible/accessible label and a disabled flag.
#[derive(Clone)]
pub struct MenuEntry {
    /// Visible text + accessible name of the row.
    pub label: String,
    /// Disabled rows are skipped by roving focus, ignore activation, and report
    /// `is_disabled` to assistive tech.
    pub disabled: bool,
}

impl MenuEntry {
    /// Create an enabled entry with the given label.
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            disabled: false,
        }
    }

    /// Mark this entry disabled.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
}

impl From<&str> for MenuEntry {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for MenuEntry {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

/// A keyboard-navigable vertical menu of [`MenuEntry`] rows.
///
/// # Example
/// ```ignore
/// MenuList::new(["Open", "Save", "Close"])
///     .selected(Some(1))
///     .on_activate(move |i, _cx| println!("chose row {i}"))
/// ```
pub struct MenuList {
    /// Row specs (label + disabled).
    entries: Vec<MenuEntry>,
    /// Presentational `Text` label per row (no a11y node of its own; the
    /// `MenuItem` node carries the name).
    rows: Vec<AnyElement>,
    /// Click + Enter activation callback.
    on_activate: MenuActivate,
    /// Roving-focus start index (clamped into range; used the first time the
    /// list mounts, then `use_state` persists the live value while open).
    initial_active: usize,
    /// Currently-committed selection, shown as an accent left-bar + `is_selected`
    /// in the a11y tree. `None` for action menus (ContextMenu) with no selection.
    selected: Option<usize>,
    /// Roving-focus index resolved from `use_state` during prepaint and reused at
    /// paint (paint has no state registry access), mirroring the spike's snapshot.
    active_value: usize,
    /// Accent colour captured at build under the render observer (active-row
    /// highlight + selection bar).
    accent: [f32; 4],
    /// Per-row height in logical px.
    row_h: f32,
    pad_x: f32,
    /// Container Taffy node + per-row wrapper nodes (for resolving row + label
    /// bounds at prepaint/paint).
    last_id: Option<ElementId>,
}

/// Layout state for MenuList — the column node plus one wrapper node per row.
pub struct MenuListLayout {
    container: taffy::NodeId,
    rows: Vec<taffy::NodeId>,
}

impl MenuList {
    /// Create a menu from any iterable of [`MenuEntry`]-convertible items
    /// (`&str`, `String`, or `MenuEntry`).
    pub fn new<I, E>(items: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<MenuEntry>,
    {
        let entries: Vec<MenuEntry> = items.into_iter().map(Into::into).collect();
        let t = theme();
        let fg: [f32; 4] = t.fg.into();
        let muted: [f32; 4] = t.muted.into();
        // Rows are presentational: the MenuItem a11y node carries the name, so a
        // nested Text label must not emit its own node (would double-announce).
        let rows = entries
            .iter()
            .map(|e| {
                let color = if e.disabled { muted } else { fg };
                presentational_label(&e.label, color, t.typography.body.size)
            })
            .collect();
        Self {
            entries,
            rows,
            on_activate: Arc::new(|_, _| {}),
            initial_active: 0,
            selected: None,
            active_value: 0,
            accent: t.accent.into(),
            row_h: t.typography.body.size + t.spacing.md,
            pad_x: t.spacing.md,
            last_id: None,
        }
    }

    /// Set the click + Enter activation callback (row index + [`EventCtx`]).
    pub fn on_activate<F>(mut self, f: F) -> Self
    where
        F: Fn(usize, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_activate = Arc::new(f);
        self
    }

    /// Set the roving-focus start index (clamped into range at mount).
    pub fn initial_active(mut self, index: usize) -> Self {
        self.initial_active = index;
        self
    }

    /// Set the committed selection (accent bar + `is_selected`). `None` = none.
    pub fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }
}

impl Element for MenuList {
    type LayoutState = MenuListLayout;
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        layout::request(self, cx)
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        prepaint::build(self, bounds, layout_state, cx);
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        layout::paint(self, bounds, layout_state, cx);
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for MenuList {}

impl IntoElement for MenuList {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
