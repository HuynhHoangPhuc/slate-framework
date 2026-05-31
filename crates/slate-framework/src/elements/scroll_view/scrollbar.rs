//! Vertical scrollbar: thumb geometry, drag math, interaction colors, and the
//! draggable thumb's mouse-handler closures.
//!
//! The thumb is an overlay rect ScrollView paints on top of its (already
//! clipped) content — it is not a child element. ScrollView allocates a stable
//! `ElementId` for it, registers a hit region + the handlers built here, and
//! resolves the paint color against the same hover/press interaction snapshot
//! that powers `Div` interaction-state styling.
//!
//! Drag state (the pointer Y and content offset captured at mouse-down) lives in
//! a per-element [`StateRegistry`](crate::reactive_state::StateRegistry) slot so
//! it survives the Strategy-A whole-view rebuild that each `offset.set` triggers
//! mid-drag — the handler closures are rebuilt every frame but re-capture the
//! same persistent [`Signal`].

use std::sync::Arc;

use slate_renderer::Lpx;
use slate_renderer::scene::RectInstance;

use crate::context::{PaintCtx, PrepaintCtx};
use crate::event::{EventCtx, MouseEvent, MouseHandler, MouseHandlers};
use crate::hit_test::{CursorStyle, HitRegion};
use crate::reactive::Signal;
use crate::types::{Bounds, ElementId, Point, Size};

/// Thumb/track width in logical pixels.
pub(super) const SCROLLBAR_WIDTH: f32 = 8.0;
/// Inset from the viewport's right edge and its top/bottom edges.
pub(super) const SCROLLBAR_MARGIN: f32 = 2.0;
/// Shortest the thumb may shrink to, so very long content stays grabbable.
pub(super) const MIN_THUMB_LEN: f32 = 24.0;

/// Resting thumb fill (translucent gray, overlay-scrollbar style).
const THUMB_COLOR: [f32; 4] = [0.55, 0.55, 0.55, 0.55];
/// Thumb fill while hovered.
const THUMB_HOVER: [f32; 4] = [0.45, 0.45, 0.45, 0.75];
/// Thumb fill while pressed / dragging.
const THUMB_ACTIVE: [f32; 4] = [0.35, 0.35, 0.35, 0.9];

/// Zero-size marker so the thumb gets its own stable `ElementId` namespace via
/// [`PrepaintCtx::allocate_id`](crate::context::PrepaintCtx::allocate_id).
pub(super) struct ScrollbarThumb;

/// Drag anchor captured at thumb mouse-down, held in a persistent state slot.
#[derive(Clone, Copy, Default)]
pub(super) struct ScrollDrag {
    /// True between mouse-down on the thumb and the following mouse-up.
    pub active: bool,
    /// Pointer Y (logical px) at the moment the drag began.
    pub start_pointer_y: f32,
    /// Content scroll offset at the moment the drag began.
    pub start_offset: f32,
}

/// Computed vertical scrollbar geometry, all in absolute (window) coordinates.
pub(super) struct ScrollbarGeometry {
    /// The thumb rect to paint and hit-test.
    pub thumb: Bounds,
    /// Distance the thumb can travel along the track (`track_len - thumb_len`).
    /// `0` when the thumb fills the track (nothing to drag).
    pub track_travel: f32,
}

/// Compute the vertical scrollbar geometry for `viewport`, or `None` when the
/// content fits (`max_offset <= 0`) and there is nothing to show.
///
/// `offset` is the (already clamped) scroll offset; thumb length is proportional
/// to `viewport / content` and clamped to [`MIN_THUMB_LEN`].
pub(super) fn vertical_scrollbar(
    viewport: Bounds,
    content_height: f32,
    offset: f32,
    max_offset: f32,
) -> Option<ScrollbarGeometry> {
    if max_offset <= 0.0 || content_height <= 0.0 {
        return None;
    }
    let viewport_h = viewport.size.height;
    let track_len = (viewport_h - 2.0 * SCROLLBAR_MARGIN).max(0.0);
    if track_len <= 0.0 {
        return None;
    }

    let track_x = viewport.origin.x + viewport.size.width - SCROLLBAR_WIDTH - SCROLLBAR_MARGIN;
    let track_y = viewport.origin.y + SCROLLBAR_MARGIN;

    let ratio = (viewport_h / content_height).clamp(0.0, 1.0);
    let thumb_len = (track_len * ratio).max(MIN_THUMB_LEN).min(track_len);
    let track_travel = (track_len - thumb_len).max(0.0);
    let progress = (offset / max_offset).clamp(0.0, 1.0);
    let thumb_y = track_y + progress * track_travel;

    Some(ScrollbarGeometry {
        thumb: Bounds::new(
            Point::new(track_x, thumb_y),
            Size::new(SCROLLBAR_WIDTH, thumb_len),
        ),
        track_travel,
    })
}

/// Map a vertical thumb drag (pointer delta in logical px) to a new content
/// offset, scaling by the content/track ratio and clamping to `[0, max_offset]`.
pub(super) fn offset_from_drag(
    start_offset: f32,
    pointer_delta: f32,
    max_offset: f32,
    track_travel: f32,
) -> f32 {
    let max = max_offset.max(0.0);
    if track_travel <= 0.0 {
        return start_offset.clamp(0.0, max);
    }
    let content_per_px = max / track_travel;
    (start_offset + pointer_delta * content_per_px).clamp(0.0, max)
}

/// Resolve the thumb fill against the current interaction snapshot
/// (pressed wins over hover, mirroring the `Div` resolution order).
pub(super) fn thumb_color(cx: &PaintCtx, id: ElementId) -> [f32; 4] {
    if cx.is_pressed(id) {
        THUMB_ACTIVE
    } else if cx.is_hovered(id) {
        THUMB_HOVER
    } else {
        THUMB_COLOR
    }
}

/// Register the draggable scrollbar thumb during prepaint and return its
/// `(id, bounds)` for paint, or `None` when the content fits (no thumb).
///
/// Must be called inside the ScrollView's pushed frame and after its children
/// so the thumb's hit region wins the topmost z. `offset` is the bound scroll
/// signal; `content_height` / `clamped_offset` / `max_offset` (in that order)
/// are this frame's resolved scroll geometry.
pub(super) fn register_thumb(
    cx: &mut PrepaintCtx,
    offset: Signal<f32>,
    viewport: Bounds,
    content_height: f32,
    clamped_offset: f32,
    max_offset: f32,
) -> Option<(ElementId, Bounds)> {
    let geo = vertical_scrollbar(viewport, content_height, clamped_offset, max_offset)?;
    let thumb_id = cx.allocate_id::<ScrollbarThumb>();
    // Record the parent edge (push+pop a frame) so clicking the thumb keeps
    // focus on the ScrollView — click-to-focus walks ancestors.
    cx.push_frame(thumb_id);
    cx.pop_frame();
    // Drag anchor persists across the rebuild each `offset.set` triggers.
    let drag = cx.state_registry.use_state::<ScrollDrag>(thumb_id, ScrollDrag::default);
    cx.register_mouse_handlers(
        thumb_id,
        build_thumb_handlers(offset, drag, max_offset, geo.track_travel),
    );
    cx.register_hit_region(HitRegion::new(thumb_id, geo.thumb, 0).with_cursor(CursorStyle::Arrow));
    Some((thumb_id, geo.thumb))
}

/// Paint the thumb on top, unclipped: it lives within the viewport but is
/// neither scissored away nor scroll-translated. Call after the content's
/// `pop_clip`. Fill resolves against the hover/press interaction snapshot.
pub(super) fn paint_thumb(cx: &mut PaintCtx, thumb_id: ElementId, bounds: Bounds) {
    let color = thumb_color(cx, thumb_id);
    cx.scene.push_rect(RectInstance {
        rect: [
            Lpx(bounds.origin.x),
            Lpx(bounds.origin.y),
            Lpx(bounds.size.width),
            Lpx(bounds.size.height),
        ],
        color,
        corner_radius: Lpx(SCROLLBAR_WIDTH / 2.0),
        _pad: [0.0; 3],
    });
}

/// Build the thumb's down/move/up handlers.
///
/// Mouse-down anchors the drag (auto-capture then routes move/up to the thumb);
/// move converts pointer travel to a new offset via [`offset_from_drag`]; up
/// clears the anchor. `max_offset` / `track_travel` are this frame's geometry,
/// re-captured every rebuild.
pub(super) fn build_thumb_handlers(
    offset: Signal<f32>,
    drag: Signal<ScrollDrag>,
    max_offset: f32,
    track_travel: f32,
) -> MouseHandlers {
    let on_mouse_down = {
        let offset = offset.clone();
        let drag = drag.clone();
        Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
            drag.set(ScrollDrag {
                active: true,
                start_pointer_y: ev.position.1,
                start_offset: offset.get(),
            });
            // Consume so the press doesn't also start a content scroll/focus.
            cx.stop_propagation();
        }) as MouseHandler
    };

    let on_mouse_move = {
        let offset = offset.clone();
        let drag = drag.clone();
        Arc::new(move |ev: &MouseEvent, cx: &mut EventCtx| {
            let d = drag.get();
            if !d.active {
                return;
            }
            let next = offset_from_drag(
                d.start_offset,
                ev.position.1 - d.start_pointer_y,
                max_offset,
                track_travel,
            );
            if (next - offset.get()).abs() > f32::EPSILON {
                offset.set(next);
            }
            cx.stop_propagation();
        }) as MouseHandler
    };

    let on_mouse_up = {
        let drag = drag.clone();
        Arc::new(move |_ev: &MouseEvent, cx: &mut EventCtx| {
            let mut d = drag.get();
            if d.active {
                d.active = false;
                drag.set(d);
            }
            cx.stop_propagation();
        }) as MouseHandler
    };

    MouseHandlers {
        on_mouse_down: Some(on_mouse_down),
        on_mouse_move: Some(on_mouse_move),
        on_mouse_up: Some(on_mouse_up),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport() -> Bounds {
        Bounds::from_origin_size(0.0, 0.0, 200.0, 100.0)
    }

    #[test]
    fn no_scrollbar_when_content_fits() {
        assert!(vertical_scrollbar(viewport(), 80.0, 0.0, 0.0).is_none());
    }

    #[test]
    fn thumb_length_is_proportional_to_viewport_over_content() {
        // viewport 100, content 200 → ratio 0.5; track_len = 100 - 2*2 = 96.
        let geo = vertical_scrollbar(viewport(), 200.0, 0.0, 100.0).expect("scrollbar");
        assert_eq!(geo.thumb.size.height, 48.0);
        // Thumb pinned to the top of the track at offset 0.
        assert_eq!(geo.thumb.origin.y, SCROLLBAR_MARGIN);
        // Right-aligned inside the viewport.
        assert_eq!(
            geo.thumb.origin.x,
            200.0 - SCROLLBAR_WIDTH - SCROLLBAR_MARGIN
        );
    }

    #[test]
    fn thumb_clamps_to_minimum_length() {
        // Huge content → tiny proportional thumb, floored at MIN_THUMB_LEN.
        let geo = vertical_scrollbar(viewport(), 100_000.0, 0.0, 99_900.0).expect("scrollbar");
        assert_eq!(geo.thumb.size.height, MIN_THUMB_LEN);
    }

    #[test]
    fn thumb_sits_at_track_bottom_when_fully_scrolled() {
        let geo = vertical_scrollbar(viewport(), 200.0, 100.0, 100.0).expect("scrollbar");
        let track_len = 100.0 - 2.0 * SCROLLBAR_MARGIN;
        let expected_y = SCROLLBAR_MARGIN + (track_len - geo.thumb.size.height);
        assert!((geo.thumb.origin.y - expected_y).abs() < 1e-3);
    }

    #[test]
    fn drag_maps_pointer_travel_to_offset() {
        // track_travel 48 over max_offset 96 → 2 content px per drag px.
        let next = offset_from_drag(0.0, 10.0, 96.0, 48.0);
        assert!((next - 20.0).abs() < 1e-3);
    }

    #[test]
    fn drag_clamps_to_range() {
        assert_eq!(offset_from_drag(50.0, 1000.0, 96.0, 48.0), 96.0);
        assert_eq!(offset_from_drag(50.0, -1000.0, 96.0, 48.0), 0.0);
    }

    #[test]
    fn drag_with_no_travel_holds_offset() {
        assert_eq!(offset_from_drag(30.0, 25.0, 96.0, 0.0), 30.0);
    }
}
