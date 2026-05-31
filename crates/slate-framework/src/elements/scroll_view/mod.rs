//! ScrollView — a clipped, vertically scrollable container.
//!
//! ScrollView is the first consumer of the renderer clip path: it pushes a
//! clip layer equal to its viewport bounds, translates its children up by the
//! current scroll offset, then pops the clip so siblings paint unclipped.
//!
//! # Scroll offset is caller-owned
//!
//! The vertical offset lives in a user-owned [`Signal<f32>`](crate::reactive::Signal)
//! passed via [`ScrollView::offset`]. This is mandatory under the Strategy-A
//! whole-view rebuild model: only a signal read under the render observer (at
//! builder time) re-renders the view when scrolling sets it. An offset read
//! from element-internal state during prepaint/paint runs *outside* the
//! observer and would never trigger a repaint. Without an offset signal the
//! container is clip-only (fixed, `Overflow::Hidden`-like).
//!
//! Offset semantics: `offset` is the number of logical pixels the content is
//! scrolled **down** from the top (`0` = top). Children paint translated by
//! `-offset` in Y. The value is clamped to `[0, content_height - viewport]`
//! at paint and by the wheel/keyboard handlers.

mod builder;
mod handlers;
mod layout;
mod reveal;
mod scrollbar;
mod virtual_list;

use crate::element::{AnyElement, IntoElement, Sealed};
use crate::reactive::Signal;
use crate::style::{Overflow, Style};
use crate::types::{Bounds, ElementId};

/// A clipped, vertically scrollable flexbox container.
///
/// # Example
///
/// ```ignore
/// // `offset` is created once (outside `render`) so it survives rebuilds.
/// ScrollView::new()
///     .offset(scroll_offset.clone())
///     .style(|s| s.height(300.0).width(400.0))
///     .child(Text::new("row 0"))
///     .child(Text::new("row 1"))
/// ```
pub struct ScrollView {
    pub(super) children: Vec<AnyElement>,
    pub(super) layout_style: Style,
    /// Caller-owned vertical scroll offset (logical px, `0` = top). `None`
    /// makes the container clip-only (non-scrolling).
    pub(super) offset: Option<Signal<f32>>,
    /// Offset resolved under the render observer at builder time (subscribes
    /// the view so `offset.set` re-renders). `0.0` when no offset signal.
    pub(super) offset_value: f32,
    /// Logical pixels scrolled per discrete (non-precise) wheel notch.
    pub(super) scroll_speed: f32,
    /// Lazy uniform-height row source. When `Some`, `children` is rebuilt each
    /// `request_layout` from the current visible window and any manually-added
    /// children are ignored; when `None` the container renders `children`
    /// directly.
    pub(in crate::elements::scroll_view) virtual_list: Option<virtual_list::VirtualList>,
    /// First absolute row index materialized this frame (window start). `0` in
    /// non-virtual mode. Set in `request_layout`, reused by prepaint/paint to
    /// translate the built window into content space.
    pub(super) virtual_first: usize,
    /// Stable ElementId allocated during prepaint (available after prepaint).
    pub(super) last_id: Option<ElementId>,
}

/// Layout state for ScrollView — stores the Taffy node ID.
pub struct ScrollViewLayoutState {
    pub(super) node_id: taffy::NodeId,
}

/// Paint state for ScrollView — the clamped offset and the scrollbar thumb
/// resolved during prepaint, reused at paint.
pub struct ScrollViewPaintState {
    /// Offset after clamping to `[0, max_offset]`, in logical pixels.
    pub(super) clamped_offset: f32,
    /// The scrollbar thumb's `(id, bounds)` when the content overflows and an
    /// offset signal is bound; `None` for a non-scrolling / clip-only view.
    /// `id` resolves the hover/press fill at paint time.
    pub(super) thumb: Option<(ElementId, Bounds)>,
}

impl Default for ScrollView {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollView {
    /// Default logical pixels scrolled per discrete wheel notch (~one line at
    /// a comfortable density). Trackpad (`precise`) deltas bypass this.
    pub(super) const DEFAULT_SCROLL_SPEED: f32 = 40.0;

    /// The overflow this element forces on its layout node so Taffy treats it
    /// as a scroll viewport (fixed box, content allowed to exceed it).
    pub(super) fn viewport_overflow() -> Overflow {
        Overflow::Scroll
    }
}

impl Sealed for ScrollView {}

impl IntoElement for ScrollView {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

/// Expose the production scrollbar-thumb handlers (down/move/up) so end-to-end
/// tests can drive a real drag through `AppState::dispatch_mouse_*` (auto-capture
/// routes move/up to the thumb), rather than re-implementing the closures. The
/// persistent drag-anchor slot is backed by a private runtime here — across-frame
/// persistence is covered separately by `StateRegistry` tests. Test-only.
#[cfg(feature = "test-hooks")]
pub fn build_thumb_handlers_for_test(
    offset: Signal<f32>,
    max_offset: f32,
    track_travel: f32,
) -> crate::event::MouseHandlers {
    let drag = Signal::new(slate_reactive::Runtime::new(), scrollbar::ScrollDrag::default());
    scrollbar::build_thumb_handlers(offset, drag, max_offset, track_travel)
}
