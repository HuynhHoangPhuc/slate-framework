//! Overlay `Element` impl — absolute layout, anchor-solved translated paint.

use slate_renderer::Lpx;
use slate_renderer::scene::RectInstance;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::Element;
use crate::layout::resolve_child_bounds;
use crate::style::Position;
use crate::types::{Bounds, ElementId, LayoutId, NodeContext, Point};

use super::anchor::solve;
use super::{Overlay, OverlayEntry, OverlayLayoutState, OverlayPaintState};

/// Modal scrim color — semi-transparent black, linear premultiplied per the
/// renderer's [`RectInstance`] contract (rgb already multiplied by alpha; black
/// stays `0`).
const SCRIM_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 0.4];

impl Overlay {
    /// Absolute origin the content child paints at this frame, plus the delta
    /// from the overlay node's laid-out origin. The child's Taffy bounds are
    /// relative to `node_origin`; re-basing by this delta lands it at the
    /// anchor-solved position instead of wherever Taffy parked the absolute node.
    fn paint_translation(&self, node_origin: Point, solved: Point) -> Point {
        Point::new(solved.x - node_origin.x, solved.y - node_origin.y)
    }
}

impl Element for Overlay {
    type LayoutState = OverlayLayoutState;
    type PaintState = OverlayPaintState;

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        // Remove the overlay from sibling flow: its painted position comes from
        // the anchor solver, never from where layout would place it.
        self.layout_style.position = Position::Absolute;

        let child_node = self.content.as_mut().map(|c| c.request_layout(cx).0);
        let taffy_style = taffy::Style::from(&self.layout_style);

        let node_id = match child_node {
            Some(cn) => cx.taffy.new_with_children(taffy_style, &[cn]),
            None => cx.taffy.new_leaf(taffy_style),
        }
        .unwrap_or_else(|e| {
            log::error!("Overlay: failed to create Taffy node: {e}; rendering empty");
            cx.taffy
                .new_leaf(taffy::Style::default())
                .unwrap_or_else(|e2| {
                    log::error!("Overlay: new_leaf also failed ({e2})");
                    taffy::NodeId::from(u64::MAX)
                })
        });
        if let Err(e) = cx.taffy.set_node_context(node_id, Some(NodeContext::None)) {
            log::error!("Overlay: failed to set node context: {e}");
        }

        (LayoutId(node_id), OverlayLayoutState { node_id })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        let node = layout_state.node_id;
        let element_id = cx.allocate_id::<Overlay>();
        self.last_id = Some(element_id);

        // Anchor-solve from the measured content size (the overlay node sizes to
        // its content) against the viewport, then register the content's hit
        // regions at that solved position so input matches the painted pixels.
        let solved = solve(self.anchor, bounds.size, self.placement, cx.viewport, self.gap);
        let translate = self.paint_translation(bounds.origin, solved);

        cx.push_frame(element_id);
        // Modal: bracket the content so its focusables are trap-scoped to this
        // overlay (keyboard focus cannot escape while it is the deepest modal).
        if self.modal {
            cx.enter_focus_trap(element_id, self.depth);
        }
        if let Some(content) = self.content.as_mut()
            && let Some(mut cb) = resolve_child_bounds(cx.taffy, node, 0, bounds.origin)
        {
            cb.origin.x += translate.x;
            cb.origin.y += translate.y;
            content.prepaint(cb, cx);
        }
        if self.modal {
            cx.exit_focus_trap();
        }
        cx.pop_frame();

        // Register as an open overlay so the dispatch path can route Esc /
        // outside-click dismissal here. The content rect is the solved origin +
        // the measured content size (the overlay node sizes to its content).
        cx.register_overlay(OverlayEntry {
            id: element_id,
            content_bounds: Bounds::from_origin_size(
                solved.x,
                solved.y,
                bounds.size.width,
                bounds.size.height,
            ),
            anchor_bounds: self.anchor,
            modal: self.modal,
            depth: self.depth,
            dismiss: self.on_dismiss.clone(),
        });

        OverlayPaintState {
            solved_origin: solved,
            viewport: cx.viewport,
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
        let translate = self.paint_translation(bounds.origin, paint_state.solved_origin);

        // Paint content into a high-depth, unclipped layer: on top of the base
        // tree and free of any ancestor scroll-clip.
        cx.push_overlay_layer(self.depth);
        // Modal scrim: a full-viewport dim drawn first into the overlay layer so
        // it sits below the content but above everything at lower depth.
        if self.modal {
            let vp = paint_state.viewport;
            cx.scene.push_rect(RectInstance {
                rect: [Lpx(0.0), Lpx(0.0), Lpx(vp.width), Lpx(vp.height)],
                color: SCRIM_COLOR,
                corner_radius: Lpx(0.0),
                _pad: [0.0; 3],
            });
        }
        if let Some(content) = self.content.as_mut()
            && let Some(mut cb) = resolve_child_bounds(cx.taffy, node, 0, bounds.origin)
        {
            cb.origin.x += translate.x;
            cb.origin.y += translate.y;
            content.paint(cb, cx);
        }
        cx.pop_overlay_layer();
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}
