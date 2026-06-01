//! Slider painting + accessibility-node construction, split from `mod.rs` to
//! keep each file focused. A child module reads the parent `Slider`'s private
//! fields directly.

use crate::context::PaintCtx;
use crate::types::{
    AccessibilityAction, AccessibilityInfo, AccessibilityNode, AccessibilityRole, Bounds, ElementId,
};

use crate::elements::control_paint::{interactive_fill, push_rect};
use super::{Slider, THUMB, THUMB_R, TRACK_THICK};

/// Paint the unfilled track, the filled portion up to the thumb, and the thumb
/// (an accent ring around a surface centre). Disabled mutes every layer.
pub(super) fn render(s: &Slider, bounds: Bounds, cx: &mut PaintCtx) {
    let (track_x, track_w) = s.track_geometry(bounds);
    let y_center = bounds.origin.y + bounds.size.height * 0.5;
    let frac = s.range.fraction(s.value_now);
    let thumb_cx = track_x + frac * track_w;

    let (track_color, fill_color) = if s.disabled {
        (s.muted, s.muted)
    } else {
        (s.track, s.accent)
    };
    push_rect(
        cx,
        Bounds::from_origin_size(track_x, y_center - TRACK_THICK * 0.5, track_w, TRACK_THICK),
        track_color,
        TRACK_THICK * 0.5,
    );
    push_rect(
        cx,
        Bounds::from_origin_size(
            track_x,
            y_center - TRACK_THICK * 0.5,
            (thumb_cx - track_x).max(0.0),
            TRACK_THICK,
        ),
        fill_color,
        TRACK_THICK * 0.5,
    );

    let ring = if s.disabled {
        s.muted
    } else {
        s.last_id
            .map_or(s.accent, |id| interactive_fill(cx, id, s.accent))
    };
    push_rect(
        cx,
        Bounds::from_origin_size(thumb_cx - THUMB_R, y_center - THUMB_R, THUMB, THUMB),
        ring,
        THUMB_R,
    );
    let inset = 2.0;
    push_rect(
        cx,
        Bounds::from_origin_size(
            thumb_cx - THUMB_R + inset,
            y_center - THUMB_R + inset,
            THUMB - 2.0 * inset,
            THUMB - 2.0 * inset,
        ),
        s.thumb_fill,
        THUMB_R - inset,
    );
}

/// Build the leaf accessibility node: role + label + value string + the
/// Increment/Decrement/Focus actions a screen reader drives.
pub(super) fn a11y_node(s: &Slider, id: ElementId, bounds: Bounds) -> AccessibilityNode {
    AccessibilityNode {
        id,
        bounds,
        info: AccessibilityInfo {
            role: AccessibilityRole::Slider,
            label: Some(s.label.clone()),
            value: Some(fmt_value(s.value_now)),
            is_disabled: s.disabled,
            ..Default::default()
        },
        children: Vec::new(),
        actions: vec![
            AccessibilityAction::Focus,
            AccessibilityAction::Increment,
            AccessibilityAction::Decrement,
        ],
    }
}

/// Format the value for screen readers: integral values without a fraction,
/// otherwise two decimals.
fn fmt_value(v: f32) -> String {
    if v.fract().abs() < 1e-4 {
        format!("{}", v as i64)
    } else {
        format!("{v:.2}")
    }
}
