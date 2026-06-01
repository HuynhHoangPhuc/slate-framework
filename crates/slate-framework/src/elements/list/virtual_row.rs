//! One materialized row of a [`VirtualList`](super::VirtualList).
//!
//! Unlike [`List`](super::List) — a single element that allocates every row id
//! so its container handler can rove real focus — a virtualized list only
//! builds the rows inside the viewport, so each row is **self-describing**: it
//! allocates its own stable id (keyed by absolute index), registers its own
//! `ListItem` accessibility node (carrying the zero-based absolute `row_index`
//! and `is_selected`), is individually Tab-focusable, and commits the
//! caller-owned selection on click. Theme colours are captured by the
//! [`VirtualList`](super::VirtualList) builder under the render observer and
//! handed in, because rows are built during the layout pass (outside the
//! observer) where `theme()` would otherwise read defaults.

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{AnyElement, Element, IntoElement, Sealed};
use crate::elements::control_paint::{presentational_label, push_rect};
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::layout::resolve_child_bounds;
use crate::reactive::Signal;
use crate::style::{Length, Style};
use crate::types::{Bounds, Edges, ElementId, LayoutId, NodeContext};

use super::ListActivate;
use super::core::list_item_node;
use super::keys::click_handler;

/// Theme colours + metrics captured under the render observer and shared by
/// every materialized [`VirtualRow`].
#[derive(Clone, Copy)]
pub(super) struct RowStyle {
    pub fg: [f32; 4],
    pub muted: [f32; 4],
    pub accent: [f32; 4],
    pub font_size: f32,
    pub row_h: f32,
    pub pad_x: f32,
}

/// A single windowed list row (see module docs).
pub(super) struct VirtualRow {
    label: String,
    row_index: usize,
    disabled: bool,
    selected: bool,
    selection: Signal<Option<usize>>,
    on_activate: ListActivate,
    style: RowStyle,
    label_elem: AnyElement,
    last_id: Option<ElementId>,
}

impl VirtualRow {
    pub(super) fn new(
        label: String,
        row_index: usize,
        disabled: bool,
        selected: bool,
        selection: Signal<Option<usize>>,
        on_activate: ListActivate,
        style: RowStyle,
    ) -> Self {
        let color = if disabled { style.muted } else { style.fg };
        let label_elem = presentational_label(&label, color, style.font_size);
        Self {
            label,
            row_index,
            disabled,
            selected,
            selection,
            on_activate,
            style,
            label_elem,
            last_id: None,
        }
    }
}

/// Layout state for a [`VirtualRow`] — the padded row node wrapping its label.
pub(super) struct VirtualRowLayout {
    row: taffy::NodeId,
}

impl Element for VirtualRow {
    type LayoutState = VirtualRowLayout;
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        let label_id = self.label_elem.request_layout(cx).0;
        let row_style = Style::new()
            .height(self.style.row_h)
            .align_items(taffy::style::AlignItems::Center)
            .padding(Edges::new(
                Length::Px(0.0),
                Length::Px(self.style.pad_x),
                Length::Px(0.0),
                Length::Px(self.style.pad_x),
            ));
        let row = cx
            .taffy
            .new_with_children(taffy::Style::from(&row_style), &[label_id])
            .unwrap_or_else(|e| {
                log::error!("VirtualRow: node failed ({e})");
                cx.taffy.new_leaf(taffy::Style::default()).unwrap_or(taffy::NodeId::from(u64::MAX))
            });
        if let Err(e) = cx.taffy.set_node_context(row, Some(NodeContext::None)) {
            log::error!("VirtualRow: set_node_context failed: {e}");
        }
        (LayoutId(row), VirtualRowLayout { row })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        // Stable id keyed by absolute index: the same row keeps its id across
        // frames while it stays in the window, so focus tracking holds.
        cx.set_next_key(format!("vrow-{}", self.row_index));
        let id = cx.allocate_id::<VirtualRow>();
        self.last_id = Some(id);

        // Prepaint the presentational label (no a11y node of its own, but paint
        // requires the ElementState to have been prepainted first).
        if let Some(lb) = resolve_child_bounds(cx.taffy, layout.row, 0, bounds.origin) {
            self.label_elem.prepaint(lb, cx);
        }

        cx.register_a11y_node(list_item_node(
            id,
            bounds,
            &self.label,
            self.row_index,
            Some(self.selected),
            self.disabled,
        ));
        if !self.disabled {
            // Tab-reachable (tab_index 0): a windowed list can't rove a single
            // cursor across un-built rows, so each visible row is its own tab
            // stop and click target.
            cx.register_focusable(FocusableEntry { id, tab_index: 0, focus_ring: true }, bounds, 4.0);
            cx.register_handlers(
                id,
                click_handler(self.row_index, self.selection.clone(), self.on_activate.clone()),
            );
            cx.register_hit_region(HitRegion::new(id, bounds, 0).with_cursor(CursorStyle::Arrow));
        }
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        layout: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        if self.selected {
            let bar = Bounds::from_origin_size(
                bounds.origin.x,
                bounds.origin.y + 4.0,
                3.0,
                (bounds.size.height - 8.0).max(0.0),
            );
            push_rect(cx, bar, self.style.accent, 1.5);
        }
        if let Some(lb) = resolve_child_bounds(cx.taffy, layout.row, 0, bounds.origin) {
            self.label_elem.paint(lb, cx);
        }
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for VirtualRow {}

impl IntoElement for VirtualRow {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
