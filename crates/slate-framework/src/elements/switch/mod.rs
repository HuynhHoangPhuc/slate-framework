//! Switch — a boolean control with the same semantics as [`Checkbox`] but a
//! sliding pill presentation and `AccessibilityRole::Switch`.
//!
//! Like Checkbox, state is caller-owned (Strategy A) and toggled on click /
//! Space via [`control_paint::toggle_handlers`](super::control_paint). The
//! visuals differ: a rounded track whose fill flips accent/border with the
//! state, and a circular thumb that slides between the two ends.
//!
//! [`Checkbox`]: super::Checkbox

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{AnyElement, Element, IntoElement, Sealed};
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::layout::resolve_child_bounds;
use crate::reactive::Signal;
use crate::style::{Length, Style};
use crate::theme::theme;
use crate::types::{
    AccessibilityInfo, AccessibilityRole, Bounds, Edges, ElementId, LayoutId, NodeContext,
};

use super::control_paint::{interactive_fill, presentational_label, push_rect, toggle_handlers};

/// Track width / height and thumb diameter, in logical pixels.
const TRACK_W: f32 = 36.0;
const TRACK_H: f32 = 20.0;
const THUMB: f32 = 16.0;
/// Inset of the thumb from the track edge.
const THUMB_INSET: f32 = (TRACK_H - THUMB) * 0.5;

/// A boolean switch bound to a caller-owned [`Signal<bool>`].
///
/// # Example
/// ```ignore
/// let on = Signal::new(cx.runtime(), true);
/// Switch::new(on.clone(), "Dark mode")
/// ```
pub struct Switch {
    value: Signal<bool>,
    on: bool,
    label: String,
    disabled: bool,
    accent: [f32; 4],
    thumb_color: [f32; 4],
    border: [f32; 4],
    muted: [f32; 4],
    gap: f32,
    text: Option<AnyElement>,
    last_id: Option<ElementId>,
}

/// Layout state for Switch — its Taffy node id.
pub struct SwitchLayout {
    node_id: taffy::NodeId,
}

impl Switch {
    /// Create a switch bound to `value` with the given visible/accessible label.
    pub fn new(value: Signal<bool>, label: impl Into<String>) -> Self {
        let label = label.into();
        let t = theme();
        let on = value.get(); // subscribe the render observer (Strategy A)
        Switch {
            value,
            on,
            label: label.clone(),
            disabled: false,
            accent: t.accent.into(),
            thumb_color: t.bg.into(),
            border: t.border.into(),
            muted: t.muted.into(),
            gap: t.spacing.sm,
            text: Some(presentational_label(&label, t.fg.into(), t.typography.body.size)),
            last_id: None,
        }
    }

    /// Mark the switch disabled (no toggle, no focus, muted track).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        if disabled {
            let t = theme();
            self.text = Some(presentational_label(
                &self.label,
                t.muted.into(),
                t.typography.body.size,
            ));
        }
        self
    }

    /// Vertically-centred track rect at the left of `bounds`.
    fn track_bounds(&self, bounds: Bounds) -> Bounds {
        Bounds::from_origin_size(
            bounds.origin.x,
            bounds.origin.y + (bounds.size.height - TRACK_H) * 0.5,
            TRACK_W,
            TRACK_H,
        )
    }
}

impl Element for Switch {
    type LayoutState = SwitchLayout;
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        let mut children = Vec::new();
        if let Some(text) = self.text.as_mut() {
            children.push(text.request_layout(cx).0);
        }
        let style = Style::new()
            .align_items(taffy::style::AlignItems::Center)
            .padding(Edges::new(
                Length::Px(0.0),
                Length::Px(0.0),
                Length::Px(0.0),
                Length::Px(TRACK_W + self.gap),
            ));
        let node_id = cx
            .taffy
            .new_with_children(taffy::Style::from(&style), &children)
            .unwrap_or_else(|e| {
                log::error!("Switch: new_with_children failed ({e})");
                cx.taffy
                    .new_leaf(taffy::Style::default())
                    .unwrap_or(taffy::NodeId::from(u64::MAX))
            });
        if let Err(e) = cx.taffy.set_node_context(node_id, Some(NodeContext::None)) {
            log::error!("Switch: set_node_context failed: {e}");
        }
        (LayoutId(node_id), SwitchLayout { node_id })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        let id = cx.allocate_id::<Switch>();
        self.last_id = Some(id);

        if !self.disabled {
            let (handlers, key_handlers) = toggle_handlers(self.value.clone());
            cx.register_handlers(id, handlers);
            cx.register_key_handlers(id, key_handlers);
            cx.register_focusable(
                FocusableEntry {
                    id,
                    tab_index: 0,
                    focus_ring: true,
                },
                bounds,
                TRACK_H * 0.5,
            );
        }
        cx.register_hit_region(HitRegion::new(id, bounds, 0).with_cursor(CursorStyle::Arrow));

        cx.prepaint_node_open(id, bounds, self.accessibility().expect("switch a11y"));
        cx.push_frame(id);
        if let Some(text) = self.text.as_mut()
            && let Some(child_bounds) =
                resolve_child_bounds(cx.taffy, layout_state.node_id, 0, bounds.origin)
        {
            text.prepaint(child_bounds, cx);
        }
        cx.pop_frame();
        cx.prepaint_node_close();
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        let track = self.track_bounds(bounds);
        // Track: accent (shaded by interaction) when on, bordered when off.
        let track_color = if self.disabled {
            self.muted
        } else if self.on {
            self.last_id
                .map_or(self.accent, |id| interactive_fill(cx, id, self.accent))
        } else {
            self.border
        };
        push_rect(cx, track, track_color, TRACK_H * 0.5);

        // Thumb slides to the right end when on, left end when off.
        let thumb_x = if self.on {
            track.origin.x + TRACK_W - THUMB - THUMB_INSET
        } else {
            track.origin.x + THUMB_INSET
        };
        let thumb = Bounds::from_origin_size(
            thumb_x,
            track.origin.y + THUMB_INSET,
            THUMB,
            THUMB,
        );
        push_rect(cx, thumb, self.thumb_color, THUMB * 0.5);

        if let Some(text) = self.text.as_mut()
            && let Some(child_bounds) =
                resolve_child_bounds(cx.taffy, layout_state.node_id, 0, bounds.origin)
        {
            text.paint(child_bounds, cx);
        }
    }

    fn accessibility(&self) -> Option<AccessibilityInfo> {
        Some(AccessibilityInfo {
            role: AccessibilityRole::Switch,
            label: Some(self.label.clone()),
            is_disabled: self.disabled,
            is_selected: Some(self.on),
            ..Default::default()
        })
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for Switch {}

impl IntoElement for Switch {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
