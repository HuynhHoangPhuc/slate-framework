//! Data-display list family: [`List`], [`VirtualList`], and the shared
//! [`ListEntry`] row spec.
//!
//! [`List`] is a vertical, keyboard-navigable column of selectable rows — the
//! one-dimensional, caller-selectable sibling of [`MenuList`](crate::MenuList).
//! It uses the same **roving-focus** model the S0 DataGrid spike validated (the
//! container is the tab stop; arrow keys move real focus to the neighbouring row
//! so a screen reader follows), reduced to one dimension, and emits a `List` →
//! `ListItem` accessibility subtree.
//!
//! [`VirtualList`] keeps the same builder surface but renders only the rows
//! inside the viewport (backed by [`ScrollView::virtualized`](crate::ScrollView::virtualized)),
//! so a 100k-row list costs the same as a visible handful while still reporting
//! the true total `row_count` to assistive tech.
//!
//! ## Selection is caller-owned; roving focus is transient
//!
//! The committed selection is a caller-owned `Signal<Option<usize>>` (Strategy
//! A: the handler writes, the next whole-view rebuild reads) — create it outside
//! `render` so it survives rebuilds. The roving "active" row (which row the
//! focus ring sits on) is *transient* per-mount UI state in a
//! [`use_state`](crate::reactive_state::StateRegistry::use_state) slot keyed by
//! the list id, initialised to the current selection.

mod core;
mod keys;
mod layout;
mod prepaint;
mod virtual_list;
mod virtual_row;

use std::sync::Arc;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{AnyElement, Element, IntoElement, Sealed};
use crate::elements::control_paint::presentational_label;
use crate::event::EventCtx;
use crate::reactive::Signal;
use crate::theme::theme;
use crate::types::{Bounds, ElementId, LayoutId};

pub use core::ListEntry;
pub use virtual_list::VirtualList;

/// Activation callback fired on Enter / row click, *after* the selection signal
/// is written. Receives the row index and the [`EventCtx`].
pub(crate) type ListActivate = Arc<dyn Fn(usize, &mut EventCtx) + Send + Sync + 'static>;

/// A vertical, keyboard-navigable list of selectable rows bound to a
/// caller-owned `Signal<Option<usize>>` selection.
///
/// # Example
/// ```ignore
/// // `selected` is created once (outside `render`) so it survives rebuilds.
/// List::new(["CPU", "Memory", "Disk"], selected.clone())
///     .label("Resources")
///     .on_activate(|i, _cx| log::info!("activated row {i}"))
/// ```
pub struct List {
    /// Row specs (label + disabled).
    entries: Vec<ListEntry>,
    /// Presentational `Text` label per row (the `ListItem` node carries the
    /// accessible name, so the label emits no a11y node of its own).
    rows: Vec<AnyElement>,
    /// Caller-owned committed selection.
    selected: Signal<Option<usize>>,
    /// Selection resolved under the render observer at build time, reused by
    /// prepaint (roving start) and paint (selection bar).
    selected_value: Option<usize>,
    /// Roving-focus index resolved from `use_state` during prepaint and reused
    /// at paint (paint has no state-registry access).
    active_value: usize,
    /// Fired on commit (Enter / click), after the selection signal is written.
    on_activate: ListActivate,
    /// Optional accessible name for the list container.
    label: Option<String>,
    /// Accent colour captured at build (active-row highlight + selection bar).
    accent: [f32; 4],
    /// Per-row height + horizontal padding in logical px.
    row_h: f32,
    pad_x: f32,
    last_id: Option<ElementId>,
}

/// Layout state for [`List`] — the column node plus one wrapper node per row.
pub struct ListLayout {
    container: taffy::NodeId,
    rows: Vec<taffy::NodeId>,
}

impl List {
    /// Create a list from any iterable of [`ListEntry`]-convertible items
    /// (`&str`, `String`, or [`ListEntry`]), bound to a caller-owned selection.
    pub fn new<I, E>(items: I, selected: Signal<Option<usize>>) -> Self
    where
        I: IntoIterator<Item = E>,
        E: Into<ListEntry>,
    {
        let entries: Vec<ListEntry> = items.into_iter().map(Into::into).collect();
        let t = theme();
        let fg: [f32; 4] = t.fg.into();
        let muted: [f32; 4] = t.muted.into();
        let rows = entries
            .iter()
            .map(|e| {
                let color = if e.disabled { muted } else { fg };
                presentational_label(&e.label, color, t.typography.body.size)
            })
            .collect();
        let selected_value = selected.get(); // subscribe the render observer
        Self {
            entries,
            rows,
            selected,
            selected_value,
            active_value: 0,
            on_activate: Arc::new(|_, _| {}),
            label: None,
            accent: t.accent.into(),
            row_h: t.typography.body.size + t.spacing.md,
            pad_x: t.spacing.md,
            last_id: None,
        }
    }

    /// Set the activation callback (row index + [`EventCtx`]), fired on Enter /
    /// click after the selection signal is written.
    pub fn on_activate<F>(mut self, f: F) -> Self
    where
        F: Fn(usize, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_activate = Arc::new(f);
        self
    }

    /// Set the accessible name announced for the list container.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }
}

impl Element for List {
    type LayoutState = ListLayout;
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

impl Sealed for List {}

impl IntoElement for List {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
