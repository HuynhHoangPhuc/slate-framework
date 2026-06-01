//! Tooltip — a hover-triggered, delayed transient label anchored to a target.
//!
//! Wraps a target element; while the pointer rests over it for
//! [`delay`](Tooltip::delay), a small label bubble is painted on the overlay
//! layer just above (or below, near the top edge) the target. It is purely
//! presentational — it registers no focusable, steals no focus, and blocks no
//! input — and hides as soon as the pointer leaves.
//!
//! ## Timing model
//!
//! The hover-start instant lives in a caller-owned `Signal<Option<Instant>>`
//! (create it outside `render`). The pointer-enter handler stamps it with the
//! current time; pointer-leave clears it. The constructor reads that signal
//! **under the render observer** so a stamp/clear rebuilds the view, and computes
//! whether the delay has elapsed. While hovering-but-not-yet-due, `paint` arms a
//! redraw at the due instant (the caret-blink pattern) so the bubble appears
//! without further input.
//!
//! ## Accessibility
//!
//! The wrapped subtree is grouped and `described_by` a persistent `Tooltip`-role
//! node carrying the label, so assistive tech can read the description whether or
//! not the bubble is visually shown.

mod paint;

use std::time::{Duration, Instant};

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{AnyElement, Element, IntoElement, Sealed};
use crate::elements::Text;
use crate::reactive::Signal;
use crate::theme::theme;
use crate::types::{AccessibilityInfo, AccessibilityRole, Bounds, ElementId, LayoutId, Size};

/// Default hover dwell before the bubble appears.
const DEFAULT_DELAY: Duration = Duration::from_millis(500);

/// Resolve the visibility + redraw-arm state from a hover-start instant.
///
/// Returns `(show, due_at)`: `show` is true once the dwell has elapsed; `due_at`
/// is `Some(instant)` only while hovering-but-not-yet-due (the moment to arm a
/// redraw at), and `None` otherwise. Pure so it is unit-tested without a window.
fn due_state(since: Option<Instant>, now: Instant, delay: Duration) -> (bool, Option<Instant>) {
    match since {
        Some(s) if now.saturating_duration_since(s) >= delay => (true, None),
        Some(s) => (false, Some(s + delay)),
        None => (false, None),
    }
}

/// A hover-triggered transient label bubble for a target element.
///
/// # Example
/// ```ignore
/// // `hover` is a caller-owned `Signal<Option<Instant>>` created outside render.
/// Tooltip::new(hover.clone(), "Delete permanently")
///     .child(IconButton::new(trash_icon, "Delete"))
/// ```
pub struct Tooltip {
    /// Wrapped target (the thing hovered over).
    target: Option<AnyElement>,
    /// The bubble label, painted + exposed to a11y.
    label: String,
    /// Presentational label `Text` (painted manually inside the bubble).
    label_text: AnyElement,
    /// Caller-owned hover-start instant (`None` = not hovering).
    hover_since: Signal<Option<Instant>>,
    /// `true` once the dwell has elapsed this frame (computed at construction).
    show: bool,
    /// `Some` while hovering-but-not-yet-due — the instant to arm a redraw at.
    due_at: Option<Instant>,
    /// Bubble colours (captured at build under the render observer).
    bubble_bg: [f32; 4],
    pad: f32,
    radius: f32,
    /// Viewport captured at prepaint, used to clamp the bubble at paint.
    viewport: Size,
    /// Container + absolute label node (for measuring the label).
    last_id: Option<ElementId>,
}

/// Layout state for Tooltip — container node + the absolute label wrapper node.
pub struct TooltipLayout {
    container: taffy::NodeId,
    label: taffy::NodeId,
}

impl Tooltip {
    /// Create a tooltip bound to a caller-owned hover-start signal, with `label`.
    /// Add the target with [`child`](Self::child).
    pub fn new(hover_since: Signal<Option<Instant>>, label: impl Into<String>) -> Self {
        let label = label.into();
        let t = theme();
        // Read under the render observer so a stamp/clear rebuilds; compute due.
        let delay = DEFAULT_DELAY;
        let (show, due_at) = due_state(hover_since.get(), Instant::now(), delay);
        Self {
            target: None,
            label: label.clone(),
            label_text: AnyElement::new(
                Text::new(label)
                    .font_size(t.typography.caption.size)
                    .color(t.bg.into())
                    .presentational(),
            ),
            hover_since,
            show,
            due_at,
            bubble_bg: t.fg.into(),
            pad: t.spacing.sm,
            radius: t.radius.sm,
            viewport: Size::ZERO,
            last_id: None,
        }
    }

    /// Override the hover dwell (default 500 ms). Recomputes the show/arm state.
    pub fn delay(mut self, delay: Duration) -> Self {
        let (show, due_at) = due_state(self.hover_since.get(), Instant::now(), delay);
        self.show = show;
        self.due_at = due_at;
        self
    }

    /// Set the wrapped target element.
    pub fn child<E: IntoElement>(mut self, target: E) -> Self {
        self.target = Some(AnyElement::new(target));
        self
    }
}

impl Element for Tooltip {
    type LayoutState = TooltipLayout;
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        paint::request(self, cx)
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        paint::prepaint(self, bounds, layout_state, cx);
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        paint::render(self, bounds, layout_state, cx);
    }

    fn accessibility(&self) -> Option<AccessibilityInfo> {
        // The group node; `described_by` is wired to the tooltip node in prepaint.
        Some(AccessibilityInfo {
            role: AccessibilityRole::Group,
            ..Default::default()
        })
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for Tooltip {}

impl IntoElement for Tooltip {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::due_state;
    use std::time::{Duration, Instant};

    #[test]
    fn not_hovering_is_hidden_and_unarmed() {
        let (show, due) = due_state(None, Instant::now(), Duration::from_millis(500));
        assert!(!show);
        assert!(due.is_none());
    }

    #[test]
    fn hovering_before_delay_is_hidden_but_armed() {
        let now = Instant::now();
        let since = now - Duration::from_millis(100);
        let (show, due) = due_state(Some(since), now, Duration::from_millis(500));
        assert!(!show, "100ms < 500ms delay → not shown yet");
        assert_eq!(due, Some(since + Duration::from_millis(500)), "armed at due instant");
    }

    #[test]
    fn hovering_past_delay_is_shown_and_unarmed() {
        let now = Instant::now();
        let since = now - Duration::from_millis(600);
        let (show, due) = due_state(Some(since), now, Duration::from_millis(500));
        assert!(show, "600ms ≥ 500ms delay → shown");
        assert!(due.is_none(), "no redraw to arm once shown");
    }
}
