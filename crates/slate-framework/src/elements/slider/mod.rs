//! Slider — a continuous/stepped value control bound to a caller-owned
//! `Signal<f32>`.
//!
//! State is caller-owned (Strategy A): the constructor reads the signal under
//! the render observer; pointer drag ([`handlers`]) and Arrow/Home/End keys
//! write it. Value arithmetic (clamp, step-snap, pixel↔value) lives in
//! [`value`] and is unit-tested there. Accessibility exposes
//! `AccessibilityRole::Slider`, the current value string, and the
//! `Increment`/`Decrement` actions screen readers drive.

mod handlers;
mod paint;
mod value;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{Element, IntoElement, Sealed};
use crate::event::KeyHandlers;
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::reactive::Signal;
use crate::style::Style;
use crate::theme::theme;
use crate::types::{Bounds, ElementId, LayoutId, NodeContext};

use handlers::SliderDrag;
use value::SliderRange;

/// Thumb diameter, track thickness, element height, and default width (px).
pub(super) const THUMB: f32 = 16.0;
pub(super) const THUMB_R: f32 = THUMB * 0.5;
pub(super) const TRACK_THICK: f32 = 4.0;
const SLIDER_H: f32 = 20.0;
const DEFAULT_W: f32 = 160.0;

/// A horizontal slider bound to a caller-owned [`Signal<f32>`].
///
/// # Example
/// ```ignore
/// let v = Signal::new(cx.runtime(), 50.0);
/// Slider::new(v.clone(), 0.0, 100.0).step(5.0).label("Volume")
/// ```
pub struct Slider {
    value: Signal<f32>,
    /// `value` snapshot for this frame's paint + a11y.
    value_now: f32,
    range: SliderRange,
    label: String,
    disabled: bool,
    width: f32,
    /// Unfilled-track colour (theme `border`).
    track: [f32; 4],
    /// Filled track + thumb ring (theme `accent`).
    accent: [f32; 4],
    /// Thumb centre fill (theme `surface`).
    thumb_fill: [f32; 4],
    /// Disabled colour (theme `muted`).
    muted: [f32; 4],
    last_id: Option<ElementId>,
}

impl Slider {
    /// Create a slider over `[min, max]` bound to `value`. Continuous by default;
    /// call [`step`](Self::step) to snap.
    pub fn new(value: Signal<f32>, min: f32, max: f32) -> Self {
        let t = theme();
        let value_now = value.get(); // subscribe the render observer (Strategy A)
        Slider {
            value,
            value_now,
            range: SliderRange {
                min,
                max,
                step: 0.0,
            },
            label: "Slider".to_string(),
            disabled: false,
            width: DEFAULT_W,
            track: t.border.into(),
            accent: t.accent.into(),
            thumb_fill: t.surface.into(),
            muted: t.muted.into(),
            last_id: None,
        }
    }

    /// Set the snap granularity (`<= 0` keeps it continuous).
    pub fn step(mut self, step: f32) -> Self {
        self.range.step = step;
        self
    }

    /// Set the accessible name (announced by screen readers).
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Set the track width in logical pixels.
    pub fn width(mut self, width: f32) -> Self {
        self.width = width.max(THUMB);
        self
    }

    /// Mark the slider disabled (no drag/keys/focus, muted track + thumb).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Track geometry: left edge X and usable width, insetting a thumb radius at
    /// each end so the thumb never overflows the element bounds.
    fn track_geometry(&self, bounds: Bounds) -> (f32, f32) {
        let track_x = bounds.origin.x + THUMB_R;
        let track_w = (bounds.size.width - THUMB).max(0.0);
        (track_x, track_w)
    }
}

impl Element for Slider {
    type LayoutState = ();
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        let style = Style::default().width(self.width).height(SLIDER_H);
        let node_id = cx
            .taffy
            .new_leaf(taffy::Style::from(&style))
            .unwrap_or_else(|e| {
                log::error!("Slider: new_leaf failed ({e})");
                taffy::NodeId::from(u64::MAX)
            });
        if let Err(e) = cx.taffy.set_node_context(node_id, Some(NodeContext::None)) {
            log::error!("Slider: set_node_context failed: {e}");
        }
        (LayoutId(node_id), ())
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        let id = cx.allocate_id::<Slider>();
        self.last_id = Some(id);
        let (track_x, track_w) = self.track_geometry(bounds);

        if !self.disabled {
            let drag = cx
                .state_registry
                .use_state::<SliderDrag>(id, SliderDrag::default);
            cx.register_mouse_handlers(
                id,
                handlers::build_mouse_handlers(
                    self.value.clone(),
                    drag,
                    self.range,
                    track_x,
                    track_w,
                ),
            );
            cx.register_key_handlers(
                id,
                KeyHandlers {
                    on_key_down: Some(handlers::build_key_handler(self.value.clone(), self.range)),
                    ..Default::default()
                },
            );
            cx.register_focusable(
                FocusableEntry {
                    id,
                    tab_index: 0,
                    focus_ring: true,
                },
                bounds,
                THUMB_R,
            );
        }
        cx.register_hit_region(HitRegion::new(id, bounds, 0).with_cursor(CursorStyle::Arrow));
        cx.register_a11y_node(paint::a11y_node(self, id, bounds));
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        paint::render(self, bounds, cx);
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for Slider {}

impl IntoElement for Slider {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
