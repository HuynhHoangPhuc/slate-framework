//! Splitter `Element` impl — proportional layout, divider interaction wiring,
//! and pane/divider paint. Split from `mod.rs` (which holds the struct +
//! builders) to keep each file focused, mirroring `scroll_view/layout.rs`.

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::Element;
use crate::elements::control_paint::{interactive_fill, push_rect};
use crate::event::KeyHandlers;
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::layout::resolve_child_bounds;
use crate::style::Style;
use crate::types::{Bounds, ElementId, LayoutId, NodeContext};

use super::handlers::{self, SplitterDrag};
use super::value::SplitRange;
use super::{DIVIDER_THICK, SplitAxis, Splitter, SplitterLayout};

impl Splitter {
    /// Lay out one pane node and impose proportional sizing on it: `flex_grow`
    /// from the pane's share of the ratio, zero basis (so grow is the only
    /// sizing input), and a main-axis `min_size` clamp. An absent pane becomes
    /// an empty leaf with the same proportional style.
    fn layout_pane(&mut self, cx: &mut LayoutCtx, first: bool, grow: f32) -> taffy::NodeId {
        let min = if first { self.min_first } else { self.min_second };
        let node = match if first { self.first.as_mut() } else { self.second.as_mut() } {
            Some(pane) => pane.request_layout(cx).0,
            None => cx
                .taffy
                .new_leaf(taffy::Style::default())
                .unwrap_or(taffy::NodeId::from(u64::MAX)),
        };
        if let Ok(existing) = cx.taffy.style(node) {
            let mut st = existing.clone();
            st.flex_grow = grow.max(0.0);
            st.flex_shrink = 1.0;
            st.flex_basis = taffy::Dimension::length(0.0);
            match self.axis {
                SplitAxis::Horizontal => st.min_size.width = taffy::Dimension::length(min),
                SplitAxis::Vertical => st.min_size.height = taffy::Dimension::length(min),
            }
            if let Err(e) = cx.taffy.set_style(node, st) {
                log::error!("Splitter: failed to set pane style: {e}");
            }
        }
        node
    }
}

impl Element for Splitter {
    type LayoutState = SplitterLayout;
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        let ratio = self.ratio_now.clamp(0.0, 1.0);
        let first_node = self.layout_pane(cx, true, ratio);

        // Divider: fixed main-axis thickness, auto cross (stretched by the
        // container's Stretch alignment); never shrinks.
        let mut divider_style = taffy::Style {
            flex_shrink: 0.0,
            ..Default::default()
        };
        match self.axis {
            SplitAxis::Horizontal => {
                divider_style.size.width = taffy::Dimension::length(DIVIDER_THICK)
            }
            SplitAxis::Vertical => {
                divider_style.size.height = taffy::Dimension::length(DIVIDER_THICK)
            }
        }
        let divider_node = cx
            .taffy
            .new_leaf(divider_style)
            .unwrap_or(taffy::NodeId::from(u64::MAX));

        let second_node = self.layout_pane(cx, false, 1.0 - ratio);

        let container_style = Style::new()
            .flex_direction(match self.axis {
                SplitAxis::Horizontal => taffy::style::FlexDirection::Row,
                SplitAxis::Vertical => taffy::style::FlexDirection::Column,
            })
            .align_items(taffy::style::AlignItems::Stretch)
            .flex_grow(1.0);
        let container = cx
            .taffy
            .new_with_children(
                taffy::Style::from(&container_style),
                &[first_node, divider_node, second_node],
            )
            .unwrap_or_else(|e| {
                log::error!("Splitter: new_with_children failed ({e})");
                cx.taffy
                    .new_leaf(taffy::Style::default())
                    .unwrap_or(taffy::NodeId::from(u64::MAX))
            });
        if let Err(e) = cx.taffy.set_node_context(container, Some(NodeContext::None)) {
            log::error!("Splitter: set_node_context failed: {e}");
        }

        (LayoutId(container), SplitterLayout { container })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        let id = cx.allocate_id::<Splitter>();
        self.last_id = Some(id);
        let container = layout_state.container;

        let (main_len, origin_main) = self.main(bounds);
        let range = SplitRange {
            container: main_len,
            divider: DIVIDER_THICK,
            min_first: self.min_first,
            min_second: self.min_second,
        };
        let divider_bounds =
            resolve_child_bounds(cx.taffy, container, 1, bounds.origin).unwrap_or(bounds);

        cx.push_frame(id);

        // Divider: draggable + keyboard-resizable value control.
        let did = cx.allocate_id::<Splitter>();
        self.divider_id = Some(did);
        let drag = cx
            .state_registry
            .use_state::<SplitterDrag>(did, SplitterDrag::default);
        cx.register_mouse_handlers(
            did,
            handlers::build_mouse_handlers(self.ratio.clone(), drag, range, origin_main, self.axis),
        );
        cx.register_key_handlers(
            did,
            KeyHandlers {
                on_key_down: Some(handlers::build_key_handler(
                    self.ratio.clone(),
                    range,
                    self.axis,
                )),
                ..Default::default()
            },
        );
        cx.register_focusable(
            FocusableEntry {
                id: did,
                tab_index: 0,
                focus_ring: true,
            },
            divider_bounds,
            2.0,
        );
        let cursor = match self.axis {
            SplitAxis::Horizontal => CursorStyle::ResizeEW,
            SplitAxis::Vertical => CursorStyle::ResizeNS,
        };
        cx.register_hit_region(HitRegion::new(did, divider_bounds, 0).with_cursor(cursor));
        cx.register_a11y_node(self.a11y_node(did, divider_bounds));

        // Panes (siblings of the divider in the a11y tree).
        if let Some(first) = self.first.as_mut()
            && let Some(cb) = resolve_child_bounds(cx.taffy, container, 0, bounds.origin)
        {
            first.prepaint(cb, cx);
        }
        if let Some(second) = self.second.as_mut()
            && let Some(cb) = resolve_child_bounds(cx.taffy, container, 2, bounds.origin)
        {
            second.prepaint(cb, cx);
        }

        cx.pop_frame();
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        let container = layout_state.container;

        // Panes first, divider on top.
        if let Some(first) = self.first.as_mut()
            && let Some(cb) = resolve_child_bounds(cx.taffy, container, 0, bounds.origin)
        {
            first.paint(cb, cx);
        }
        if let Some(second) = self.second.as_mut()
            && let Some(cb) = resolve_child_bounds(cx.taffy, container, 2, bounds.origin)
        {
            second.paint(cb, cx);
        }
        if let Some(db) = resolve_child_bounds(cx.taffy, container, 1, bounds.origin) {
            let color = self
                .divider_id
                .map_or(self.divider, |id| interactive_fill(cx, id, self.divider));
            push_rect(cx, db, color, 0.0);
        }
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}
