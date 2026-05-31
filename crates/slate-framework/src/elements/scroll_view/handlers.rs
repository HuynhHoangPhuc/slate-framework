//! ScrollView scroll math + internally-built wheel/keyboard handler closures.
//!
//! The closures capture the caller-owned offset `Signal` plus the per-frame
//! scroll extents (`max_offset`, `viewport`) computed during prepaint, so a
//! handler fired on a later frame clamps against the geometry from the frame
//! it was registered on (at most one frame stale — fine for scrolling).

use std::sync::Arc;

use slate_platform::{Key, NamedKey};

use crate::event::{ElementKeyHandler, ScrollHandler};
use crate::reactive::Signal;

/// Clamp a scroll offset to the scrollable range `[0, max]`.
///
/// `max` is `content_height - viewport_height`; a negative `max` (content
/// shorter than the viewport) clamps everything to `0`.
pub(super) fn clamp_offset(value: f32, max: f32) -> f32 {
    value.clamp(0.0, max.max(0.0))
}

/// Build the wheel/trackpad scroll handler.
///
/// `delta_y > 0` means "scroll up" (content moves down) per [`ScrollEvent`],
/// which *decreases* the scrolled-down offset. Discrete wheels multiply by
/// `speed`; precise (trackpad) deltas are used as-is.
///
/// [`ScrollEvent`]: crate::event::ScrollEvent
pub(super) fn scroll_handler(offset: Signal<f32>, max: f32, speed: f32) -> ScrollHandler {
    Arc::new(move |ev, cx| {
        let step = if ev.precise {
            ev.delta_y
        } else {
            ev.delta_y * speed
        };
        let current = offset.get();
        let next = clamp_offset(current - step, max);
        if (next - current).abs() > f32::EPSILON {
            offset.set(next);
            // The scroll was consumed here; don't let an ancestor scroll too.
            cx.stop_propagation();
        }
    })
}

/// Build the keyboard scroll handler (arrows, PageUp/Down, Home/End).
///
/// Fires while the ScrollView is on the focused chain. `line` is the arrow-key
/// step; PageUp/Down move by ~90% of the viewport so a sliver of context
/// carries over between pages.
pub(super) fn key_handler(offset: Signal<f32>, max: f32, viewport: f32) -> ElementKeyHandler {
    let line = 40.0_f32;
    let page = (viewport * 0.9).max(line);
    Arc::new(move |ev, cx| {
        let current = offset.get();
        let target = match &ev.key {
            Key::Named(NamedKey::ArrowDown) => current + line,
            Key::Named(NamedKey::ArrowUp) => current - line,
            Key::Named(NamedKey::PageDown) => current + page,
            Key::Named(NamedKey::PageUp) => current - page,
            Key::Named(NamedKey::Home) => 0.0,
            Key::Named(NamedKey::End) => max,
            _ => return,
        };
        let next = clamp_offset(target, max);
        if (next - current).abs() > f32::EPSILON {
            offset.set(next);
        }
        // Consume the navigation key even at a clamp boundary so it doesn't
        // also move focus or scroll an ancestor.
        cx.stop_propagation();
    })
}

#[cfg(test)]
mod tests {
    use super::clamp_offset;

    #[test]
    fn clamp_holds_within_range() {
        assert_eq!(clamp_offset(50.0, 100.0), 50.0);
    }

    #[test]
    fn clamp_floors_at_zero() {
        assert_eq!(clamp_offset(-10.0, 100.0), 0.0);
    }

    #[test]
    fn clamp_ceils_at_max() {
        assert_eq!(clamp_offset(250.0, 100.0), 100.0);
    }

    #[test]
    fn negative_max_clamps_to_zero() {
        // Content shorter than viewport → nothing scrolls.
        assert_eq!(clamp_offset(30.0, -80.0), 0.0);
    }
}
