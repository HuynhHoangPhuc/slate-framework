//! Framework-provided focus ring overlay.
//!
//! Emitted into the scene at the end of the paint pass so it overlays element
//! content. Implemented as four thin filled rectangles (top / right / bottom /
//! left) — corner_radius is approximated as sharp on the stroke itself; the
//! underlying element keeps its rounded fill. v1 ships a hardcoded system
//! accent color; a future theme pass can replace `FOCUS_RING_COLOR`.
//!
//! Painted bounds are captured during `Div::prepaint` (only for elements that
//! opted into focus via `focusable(true)`), so the per-frame cost is bounded
//! by the number of focusable elements, not the full tree.

use slate_renderer::Lpx;
use slate_renderer::scene::{RectInstance, Scene};

use crate::types::Bounds;

/// Cached paint info for a focusable element. Captured during `Div::prepaint`
/// and consulted after the paint walk when emitting the ring overlay.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FocusBounds {
    pub bounds: Bounds,
    pub corner_radius: f32,
}

/// System accent blue (open question OQ1 resolved here). Linear,
/// premultiplied per the renderer's RectInstance color contract.
const FOCUS_RING_COLOR: [f32; 4] = [0.247, 0.518, 1.0, 1.0];

/// Stroke width in logical pixels.
const FOCUS_RING_WIDTH: f32 = 2.0;

/// Distance the outer edge of the stroke sits outside the element bounds.
const FOCUS_RING_OUTSET: f32 = 1.0;

/// Emit a 2px focus-ring stroke around `info.bounds` into `scene`.
///
/// Implemented as four filled rectangles forming the stroke; `corner_radius`
/// is set to 0 on the stroke segments. The visual approximation is good at
/// 2px even on rounded elements.
pub(crate) fn emit_focus_ring(scene: &mut Scene, info: FocusBounds) {
    let total = FOCUS_RING_OUTSET + FOCUS_RING_WIDTH;
    let x = info.bounds.origin.x - total;
    let y = info.bounds.origin.y - total;
    let w = info.bounds.size.width + 2.0 * total;
    let h = info.bounds.size.height + 2.0 * total;

    // For a rounded element the corner segments shift outward; matching
    // (corner_radius + outset) keeps the stroke flush with rounded fills.
    let outer_radius = if info.corner_radius > 0.0 {
        info.corner_radius + total
    } else {
        0.0
    };

    let pad = [0.0_f32; 3];
    let stroke = FOCUS_RING_WIDTH;

    // Top stroke
    scene.push_rect(RectInstance {
        rect: [Lpx(x), Lpx(y), Lpx(w), Lpx(stroke)],
        color: FOCUS_RING_COLOR,
        corner_radius: Lpx(0.0),
        _pad: pad,
    });
    // Bottom stroke
    scene.push_rect(RectInstance {
        rect: [Lpx(x), Lpx(y + h - stroke), Lpx(w), Lpx(stroke)],
        color: FOCUS_RING_COLOR,
        corner_radius: Lpx(0.0),
        _pad: pad,
    });
    // Left stroke
    scene.push_rect(RectInstance {
        rect: [Lpx(x), Lpx(y + stroke), Lpx(stroke), Lpx(h - 2.0 * stroke)],
        color: FOCUS_RING_COLOR,
        corner_radius: Lpx(0.0),
        _pad: pad,
    });
    // Right stroke
    scene.push_rect(RectInstance {
        rect: [
            Lpx(x + w - stroke),
            Lpx(y + stroke),
            Lpx(stroke),
            Lpx(h - 2.0 * stroke),
        ],
        color: FOCUS_RING_COLOR,
        corner_radius: Lpx(0.0),
        _pad: pad,
    });

    // Reserved: rounded-corner caps. When corner_radius > 0 the four stroke
    // rects leave 90° corners; a future refinement can replace this with
    // outer-rounded fill minus inner-rounded mask once the renderer exposes
    // a stroke primitive. v1 ships sharp-corner stroke + the outer_radius
    // value is kept here for that future path.
    let _ = outer_radius;
}
