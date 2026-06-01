//! [`DataGrid`] — a 2-D, screen-reader-usable, virtualized data grid.
//!
//! The program's headline widget and hardest accessibility surface. It adopts
//! the **S0 spike's verified role/nav pattern unchanged** (validated against
//! real VoiceOver): a `Grid` → `Row` → `ColumnHeader`/`Cell` subtree with
//! zero-based cell indices and a roving-focus active cell, extended with a real
//! column model, owned tabular data, and a windowed (virtualized) data area.
//!
//! ## Every logical cell is registered, but only the window is shaped
//!
//! Roving focus moves real focus to the neighbour cell via `request_focus`; a
//! screen reader follows the focused cell. But `request_focus` only lands on a
//! cell **registered this frame** (focus on an unregistered id is dropped, never
//! retried), so a Page/Home/End jump to an off-window row couldn't move focus if
//! only the window were materialized. So the grid registers a lightweight a11y
//! node + focusable + hit region for **every** logical cell each frame (cheap
//! struct pushes — no layout/shaping), while the expensive glyph **content** is
//! built only for the visible window. The container reports the **true total**
//! `row_count`, so VoiceOver announces "row X of N" correctly.
//!
//! State is caller-owned (Strategy A): the active `(row, col)`, scroll `offset`,
//! and optional row-selection `Signal`s are read under the render observer at
//! construction and written by the handlers — create them outside `render`.

mod columns;
mod keys;
mod layout;
mod paint;
mod prepaint;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{AnyElement, Element, IntoElement, Sealed};
use crate::reactive::Signal;
use crate::theme::theme;
use crate::types::{Bounds, ElementId, LayoutId, Point};

pub use columns::Column;

/// Theme colours + metrics captured under the render observer at construction,
/// then reused while building cell content during the layout pass (where
/// `theme()` would otherwise read defaults).
#[derive(Clone, Copy)]
pub(super) struct GridStyle {
    pub fg: [f32; 4],
    pub header_bg: [f32; 4],
    pub border: [f32; 4],
    /// Translucent accent fill for the selected row.
    pub sel_bg: [f32; 4],
}

/// A 2-D, screen-reader-usable, virtualized data grid (see module docs).
///
/// ```ignore
/// DataGrid::new(columns, rows, active.clone(), offset.clone())
///     .height(400.0).label("Processes").selected(selected.clone())
/// ```
pub struct DataGrid {
    pub(super) columns: Vec<Column>,
    pub(super) rows: Vec<Vec<String>>,
    /// Caller-owned active cell `(total_row, col)` — `total_row 0` is the header.
    pub(super) active: Signal<(usize, usize)>,
    /// Caller-owned vertical scroll offset over the data area (logical px).
    pub(super) offset: Signal<f32>,
    pub(super) offset_value: f32,
    /// Optional caller-owned selected **data**-row index (0-based, header
    /// excluded). Additive — `None` disables row selection.
    pub(super) selected: Option<Signal<Option<usize>>>,
    pub(super) selected_value: Option<usize>,
    /// Accessible name for the grid container.
    pub(super) label: Option<String>,
    /// Fixed viewport height (logical px) — windows the data rows.
    pub(super) height: f32,
    /// Uniform row height (header + data), logical px.
    pub(super) row_h: f32,
    /// Horizontal cell padding, logical px.
    pub(super) pad_x: f32,
    /// Cell-text font size, logical px.
    pub(super) font_size: f32,
    pub(super) style: GridStyle,
    /// Grid origin, set at prepaint/paint from the incoming bounds; cell
    /// arithmetic reads it.
    pub(super) origin: Point,
    /// Visible cell content elements `(total_row, col, text)` built in
    /// `request_layout` and painted at arithmetic bounds. Header + window only.
    pub(super) cells: Vec<(usize, usize, AnyElement)>,
    pub(super) last_id: Option<ElementId>,
}

impl DataGrid {
    /// Create a grid from a column spec and owned row data, bound to caller-owned
    /// `active`-cell and scroll-`offset` signals. Each row is a vector of cell
    /// strings aligned to `columns`.
    pub fn new<R, C>(
        columns: Vec<Column>,
        rows: R,
        active: Signal<(usize, usize)>,
        offset: Signal<f32>,
    ) -> Self
    where
        R: IntoIterator<Item = C>,
        C: IntoIterator<Item = String>,
    {
        let rows: Vec<Vec<String>> = rows
            .into_iter()
            .map(|r| r.into_iter().collect())
            .collect();
        let t = theme();
        let accent: [f32; 4] = t.accent.into();
        let style = GridStyle {
            fg: t.fg.into(),
            header_bg: t.surface.into(),
            border: t.border.into(),
            // Premultiplied translucent accent (rgb already premult at α=1 → scale
            // all four to drop to α≈0.22 while staying a valid premult colour).
            sel_bg: [accent[0] * 0.22, accent[1] * 0.22, accent[2] * 0.22, 0.22],
        };
        let _ = active.get(); // subscribe the render observer (Strategy A)
        let offset_value = offset.get();
        Self {
            columns,
            rows,
            active,
            offset,
            offset_value,
            selected: None,
            selected_value: None,
            label: None,
            height: 320.0,
            row_h: t.typography.body.size + t.spacing.md,
            pad_x: t.spacing.md,
            font_size: t.typography.body.size,
            style,
            origin: Point::ZERO,
            cells: Vec::new(),
            last_id: None,
        }
    }

    /// Set the fixed viewport height (logical px) that windows the data rows.
    pub fn height(mut self, height: f32) -> Self {
        self.height = height.max(0.0);
        self
    }

    /// Set the accessible name announced for the grid container.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Bind an optional caller-owned selected-row signal (0-based data-row index,
    /// header excluded). Enables row click/Enter selection + a row highlight.
    pub fn selected(mut self, selected: Signal<Option<usize>>) -> Self {
        self.selected_value = selected.get();
        self.selected = Some(selected);
        self
    }
}

impl Element for DataGrid {
    type LayoutState = ();
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        layout::request(self, cx)
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        prepaint::build(self, bounds, cx);
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        paint::render(self, bounds, cx);
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for DataGrid {}

impl IntoElement for DataGrid {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
