//! Prepaint (a11y + hover handlers) and paint (bubble + redraw arming) for
//! [`Tooltip`](super::Tooltip). Split from `mod.rs` to stay under the size budget.

use std::sync::Arc;
use std::time::Instant;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::elements::control_paint::push_rect;
use crate::event::{EventCtx, Handlers, PointerEvent, PointerHandler};
use crate::hit_test::HitRegion;
use crate::layout::resolve_child_bounds;
use crate::style::{Position, Style};
use crate::types::{
    AccessibilityInfo, AccessibilityNode, AccessibilityRelationships, AccessibilityRole, Bounds,
    LayoutId, NodeContext,
};

use super::{Tooltip, TooltipLayout};

/// Lay out the target (flow) plus the label inside an absolute wrapper (measured
/// but out of flow, so the tooltip occupies only the target's space).
pub(super) fn request(tip: &mut Tooltip, cx: &mut LayoutCtx) -> (LayoutId, TooltipLayout) {
    let mut children = Vec::new();
    if let Some(target) = tip.target.as_mut() {
        children.push(target.request_layout(cx).0);
    }
    let label_inner = tip.label_text.request_layout(cx).0;
    let abs = Style::new().position(Position::Absolute);
    let label_node = cx
        .taffy
        .new_with_children(taffy::Style::from(&abs), &[label_inner])
        .unwrap_or_else(|e| {
            log::error!("Tooltip: label node failed ({e})");
            cx.taffy.new_leaf(taffy::Style::default()).unwrap_or(taffy::NodeId::from(u64::MAX))
        });
    children.push(label_node);

    let container = cx
        .taffy
        .new_with_children(taffy::Style::default(), &children)
        .unwrap_or_else(|e| {
            log::error!("Tooltip: container failed ({e})");
            cx.taffy.new_leaf(taffy::Style::default()).unwrap_or(taffy::NodeId::from(u64::MAX))
        });
    if let Err(e) = cx.taffy.set_node_context(container, Some(NodeContext::None)) {
        log::error!("Tooltip: set_node_context failed: {e}");
    }
    (LayoutId(container), TooltipLayout { container, label: label_node })
}

/// Main-axis gap between the target and the bubble (logical px).
const GAP: f32 = 6.0;
/// Bubble z-depth: above the base tree, below dropdown/menu overlays (1000).
const DEPTH: i32 = 900;

/// Distinct marker so the tooltip a11y node id never collides with the wrapper.
struct TooltipNodeMarker;

/// Build hover handlers + the `Group` → `Tooltip` a11y subtree.
pub(super) fn prepaint(
    tip: &mut Tooltip,
    bounds: Bounds,
    layout: &mut TooltipLayout,
    cx: &mut PrepaintCtx,
) {
    tip.viewport = cx.viewport;
    let container_id = cx.allocate_id::<Tooltip>();
    tip.last_id = Some(container_id);
    let tooltip_node_id = cx.allocate_id::<TooltipNodeMarker>();

    // Hover handlers stamp / clear the caller's hover-start signal. Pointer
    // enter/leave are transient and never steal focus, so the tooltip is inert.
    let since = tip.hover_since.clone();
    let on_enter: PointerHandler =
        Arc::new(move |_ev: &PointerEvent, _cx: &mut EventCtx| since.set(Some(Instant::now())));
    let since_leave = tip.hover_since.clone();
    let on_leave: PointerHandler =
        Arc::new(move |_ev: &PointerEvent, _cx: &mut EventCtx| since_leave.set(None));
    cx.register_handlers(
        container_id,
        Handlers {
            on_pointer_enter: Some(on_enter),
            on_pointer_leave: Some(on_leave),
            ..Default::default()
        },
    );
    cx.register_hit_region(HitRegion::new(container_id, bounds, 0));

    // a11y: a Group described_by a persistent Tooltip node (label always present
    // to AT, independent of the bubble's visibility).
    let info = AccessibilityInfo {
        role: AccessibilityRole::Group,
        relationships: AccessibilityRelationships {
            described_by: vec![tooltip_node_id],
            ..Default::default()
        },
        ..Default::default()
    };
    cx.prepaint_node_open(container_id, bounds, info);
    cx.push_frame(container_id);
    cx.register_a11y_node(AccessibilityNode {
        id: tooltip_node_id,
        bounds,
        info: AccessibilityInfo {
            role: AccessibilityRole::Tooltip,
            label: Some(tip.label.clone()),
            ..Default::default()
        },
        children: Vec::new(),
        actions: Vec::new(),
    });
    if let Some(target) = tip.target.as_mut()
        && let Some(tb) = resolve_child_bounds(cx.taffy, layout.container, 0, bounds.origin)
    {
        target.prepaint(tb, cx);
    }
    cx.pop_frame();
    cx.prepaint_node_close();
}

/// Paint the target, arm the show timer while hovering, and draw the bubble once
/// the dwell has elapsed.
pub(super) fn render(tip: &mut Tooltip, bounds: Bounds, layout: &mut TooltipLayout, cx: &mut PaintCtx) {
    if let Some(target) = tip.target.as_mut()
        && let Some(tb) = resolve_child_bounds(cx.taffy, layout.container, 0, bounds.origin)
    {
        target.paint(tb, cx);
    }
    // While hovering but not yet due, ask the platform to redraw at the due
    // instant so the bubble appears without further input.
    if let Some(due) = tip.due_at {
        cx.schedule_redraw_at(due);
    }
    if !tip.show {
        return;
    }

    let (lw, lh) = cx
        .taffy
        .layout(layout.label)
        .map(|l| (l.size.width, l.size.height))
        .unwrap_or((0.0, 0.0));
    let pad = tip.pad;
    let bw = lw + 2.0 * pad;
    let bh = lh + 2.0 * pad;
    // Centred above the target; flips below near the top edge; clamped to the
    // viewport on x.
    let mut bx = bounds.origin.x + (bounds.size.width - bw) * 0.5;
    let mut by = bounds.origin.y - bh - GAP;
    if by < 0.0 {
        by = bounds.origin.y + bounds.size.height + GAP;
    }
    if tip.viewport.width > 0.0 {
        bx = bx.clamp(0.0, (tip.viewport.width - bw).max(0.0));
    }

    cx.push_overlay_layer(DEPTH);
    push_rect(cx, Bounds::from_origin_size(bx, by, bw, bh), tip.bubble_bg, tip.radius);
    let label_bounds = Bounds::from_origin_size(bx + pad, by + pad, lw, lh);
    tip.label_text.paint(label_bounds, cx);
    cx.pop_overlay_layer();
}
