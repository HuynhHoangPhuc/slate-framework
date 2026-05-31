//! ScrollView `Element` impl — layout, prepaint, clipped/translated paint.

use slate_renderer::Lpx;
use slate_renderer::scene::ClipRect;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::Element;
use crate::event::{Handlers, KeyHandlers};
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::layout::resolve_child_bounds;
use crate::types::{
    AccessibilityInfo, AccessibilityRole, Bounds, ElementId, LayoutId, NodeContext, Point,
};

use super::handlers::{clamp_offset, key_handler, scroll_handler};
use super::reveal::reveal_focused_descendant;
use super::scrollbar::{paint_thumb, register_thumb};
use super::{ScrollView, ScrollViewLayoutState, ScrollViewPaintState};

impl ScrollView {
    /// Total scrollable height of the content, in logical pixels: the bottom
    /// edge of the lowest child (measured from the node's border-box top, which
    /// already includes top border + padding) plus the node's bottom padding
    /// and border. Including the trailing inset lets a fully-scrolled view
    /// reveal it; omitting it would under-count `max_offset`. Used to bound the
    /// scroll offset.
    fn content_height(&self, cx: &PrepaintCtx, node: taffy::NodeId) -> f32 {
        let mut bottom = 0.0_f32;
        for i in 0..self.children.len() {
            if let Some(b) = resolve_child_bounds(cx.taffy, node, i, Point::ZERO) {
                bottom = bottom.max(b.origin.y + b.size.height);
            }
        }
        if let Ok(layout) = cx.taffy.layout(node) {
            bottom += layout.padding.bottom + layout.border.bottom;
        }
        bottom
    }
}

impl Element for ScrollView {
    type LayoutState = ScrollViewLayoutState;
    type PaintState = ScrollViewPaintState;

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        // Force a scroll viewport: a fixed box whose content may exceed it.
        self.layout_style.overflow = Self::viewport_overflow();

        let mut child_nodes = Vec::with_capacity(self.children.len());
        for child in &mut self.children {
            child_nodes.push(child.request_layout(cx).0);
        }

        // Scroll content must keep its natural size and overflow the viewport,
        // not flex-shrink to fit it. Pin every direct child's `flex_shrink` to
        // 0 so a column of fixed-height rows stays scrollable instead of being
        // squeezed into the fixed viewport height. This deliberately overrides
        // an explicit `flex_shrink` set on a direct child — inside a scroll
        // container "shrink to fit" is the wrong semantic. Only direct children
        // are pinned; a shrinking grandchild inside a child Div is unaffected.
        for &cn in &child_nodes {
            if let Ok(style) = cx.taffy.style(cn)
                && style.flex_shrink != 0.0
            {
                let mut pinned = style.clone();
                pinned.flex_shrink = 0.0;
                if let Err(e) = cx.taffy.set_style(cn, pinned) {
                    log::error!("ScrollView: failed to pin child flex_shrink: {e}");
                }
            }
        }

        let taffy_style = taffy::Style::from(&self.layout_style);
        let node_id = match cx.taffy.new_with_children(taffy_style, &child_nodes) {
            Ok(id) => id,
            Err(e) => {
                log::error!("ScrollView: failed to create Taffy node: {e}; rendering empty");
                cx.taffy
                    .new_leaf(taffy::Style::default())
                    .unwrap_or_else(|e2| {
                        log::error!("ScrollView: new_leaf also failed ({e2})");
                        taffy::NodeId::from(u64::MAX)
                    })
            }
        };
        if let Err(e) = cx.taffy.set_node_context(node_id, Some(NodeContext::None)) {
            log::error!("ScrollView: failed to set node context: {e}");
        }

        (LayoutId(node_id), ScrollViewLayoutState { node_id })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        let node = layout_state.node_id;
        let element_id = cx.allocate_id::<ScrollView>();
        self.last_id = Some(element_id);

        // Resolve the scrollable range, then clamp the (caller-owned) offset.
        let viewport = bounds.size.height;
        let content_height = self.content_height(cx, node);
        let max_offset = (content_height - viewport).max(0.0);
        let clamped_offset = clamp_offset(self.offset_value, max_offset);

        // Wire scroll behaviour only when an offset signal is bound; otherwise
        // the container is clip-only and stays non-interactive.
        if let Some(offset) = self.offset.clone() {
            cx.register_handlers(
                element_id,
                Handlers {
                    on_mouse_scrolled: Some(scroll_handler(
                        offset.clone(),
                        max_offset,
                        self.scroll_speed,
                    )),
                    ..Default::default()
                },
            );
            cx.register_key_handlers(
                element_id,
                KeyHandlers {
                    on_key_down: Some(key_handler(offset, max_offset, viewport)),
                    ..Default::default()
                },
            );
            // Focusable (no ring) so keyboard scrolling can target the viewport.
            cx.register_focusable(
                FocusableEntry {
                    id: element_id,
                    tab_index: 0,
                    focus_ring: false,
                },
                bounds,
                0.0,
            );
            cx.register_hit_region(
                HitRegion::new(element_id, bounds, 0).with_cursor(CursorStyle::Arrow),
            );
        }

        // a11y: open a ScrollView node so children nest beneath it.
        let opened_a11y = if let Some(info) = self.accessibility() {
            cx.prepaint_node_open(element_id, bounds, info);
            true
        } else {
            false
        };
        cx.push_frame(element_id);
        for (i, child) in self.children.iter_mut().enumerate() {
            if let Some(mut cb) = resolve_child_bounds(cx.taffy, node, i, bounds.origin) {
                cb.origin.y -= clamped_offset;
                child.prepaint(cb, cx);
            }
        }

        // Keyboard / screen-reader reveal: if focus moved to an off-screen
        // descendant this frame, scroll it into view (sets the offset signal so
        // the reveal lands next frame). Runs after the children so their parent
        // links and focus bounds are recorded; only when scrolling is enabled.
        if let Some(offset) = self.offset.clone() {
            reveal_focused_descendant(cx, element_id, &offset, bounds, clamped_offset, max_offset);
        }

        // Scrollbar thumb: a hittable, draggable overlay registered after the
        // children (topmost z), only when an offset signal is bound and the
        // content overflows.
        let thumb = self.offset.clone().and_then(|offset| {
            register_thumb(cx, offset, bounds, content_height, clamped_offset, max_offset)
        });

        cx.pop_frame();
        if opened_a11y {
            cx.prepaint_node_close();
        }

        ScrollViewPaintState {
            clamped_offset,
            thumb,
        }
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        let node = layout_state.node_id;
        let offset = paint_state.clamped_offset;

        // Clip subsequent draws to the viewport (absolute bounds — NOT the
        // scroll-translated child positions), then paint children shifted up.
        cx.push_clip(ClipRect::new(
            Lpx(bounds.origin.x),
            Lpx(bounds.origin.y),
            Lpx(bounds.size.width),
            Lpx(bounds.size.height),
        ));
        for (i, child) in self.children.iter_mut().enumerate() {
            if let Some(mut cb) = resolve_child_bounds(cx.taffy, node, i, bounds.origin) {
                cb.origin.y -= offset;
                child.paint(cb, cx);
            }
        }
        cx.pop_clip();

        if let Some((thumb_id, tb)) = paint_state.thumb {
            paint_thumb(cx, thumb_id, tb);
        }
    }

    fn accessibility(&self) -> Option<AccessibilityInfo> {
        Some(AccessibilityInfo {
            role: AccessibilityRole::ScrollView,
            ..Default::default()
        })
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}
