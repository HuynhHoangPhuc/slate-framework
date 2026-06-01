//! Checkbox — a boolean control bound to a caller-owned `Signal<bool>`.
//!
//! The checkbox paints a small square indicator followed by its visible label.
//! State is **caller-owned** (Strategy A): the constructor reads the signal under
//! the render observer so a toggle re-renders the view; the click / Space handler
//! writes it (see [`control_paint::toggle_handlers`](super::control_paint)).
//!
//! Indicator rendering: the renderer exposes only axis-aligned rects, so the
//! "checked" state is shown as a filled accent square with a centred notch rather
//! than a diagonal tick (a stroked checkmark needs the line primitive that
//! arrives with the primitive-drawn viz work). The accessible state is always
//! exact via `is_selected`, independent of the glyph.

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

/// Side length of the square indicator, in logical pixels.
const BOX_SIZE: f32 = 18.0;

/// A boolean checkbox bound to a caller-owned [`Signal<bool>`].
///
/// # Example
/// ```ignore
/// let on = Signal::new(cx.runtime(), false);
/// Checkbox::new(on.clone(), "Enable notifications")
/// ```
pub struct Checkbox {
    /// Caller-owned checked state. Read under the render observer at build.
    value: Signal<bool>,
    /// `value` snapshot for this frame's paint + a11y.
    checked: bool,
    /// Visible label, also the accessible name.
    label: String,
    disabled: bool,
    /// Resolved theme colours (captured at build under the render observer).
    accent: [f32; 4],
    surface: [f32; 4],
    border: [f32; 4],
    muted: [f32; 4],
    radius: f32,
    gap: f32,
    /// Label text child (laid out to the right of the indicator).
    text: Option<AnyElement>,
    last_id: Option<ElementId>,
}

/// Layout state for Checkbox — its Taffy node id.
pub struct CheckboxLayout {
    node_id: taffy::NodeId,
}

impl Checkbox {
    /// Create a checkbox bound to `value` with the given visible/accessible label.
    pub fn new(value: Signal<bool>, label: impl Into<String>) -> Self {
        let label = label.into();
        let t = theme();
        let checked = value.get(); // subscribe the render observer (Strategy A)
        Checkbox {
            value,
            checked,
            label: label.clone(),
            disabled: false,
            accent: t.accent.into(),
            surface: t.surface.into(),
            border: t.border.into(),
            muted: t.muted.into(),
            radius: t.radius.sm,
            gap: t.spacing.sm,
            text: Some(presentational_label(&label, t.fg.into(), t.typography.body.size)),
            last_id: None,
        }
    }

    /// Mark the checkbox disabled (no toggle, no focus, muted indicator).
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        if disabled {
            // Re-tint the label to muted to match the disabled indicator.
            let t = theme();
            self.text = Some(presentational_label(&self.label, t.muted.into(), t.typography.body.size));
        }
        self
    }

    /// Vertically-centred indicator rect at the left of `bounds`.
    fn box_bounds(&self, bounds: Bounds) -> Bounds {
        Bounds::from_origin_size(
            bounds.origin.x,
            bounds.origin.y + (bounds.size.height - BOX_SIZE) * 0.5,
            BOX_SIZE,
            BOX_SIZE,
        )
    }
}

impl Element for Checkbox {
    type LayoutState = CheckboxLayout;
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        let mut children = Vec::new();
        if let Some(text) = self.text.as_mut() {
            children.push(text.request_layout(cx).0);
        }
        // Reserve the indicator's column via left padding; the label is the only
        // Taffy child and flows to its right. Row + centre keeps both aligned.
        let style = Style::new()
            .align_items(taffy::style::AlignItems::Center)
            .padding(Edges::new(
                Length::Px(0.0),
                Length::Px(0.0),
                Length::Px(0.0),
                Length::Px(BOX_SIZE + self.gap),
            ));
        let node_id = cx
            .taffy
            .new_with_children(taffy::Style::from(&style), &children)
            .unwrap_or_else(|e| {
                log::error!("Checkbox: new_with_children failed ({e})");
                cx.taffy
                    .new_leaf(taffy::Style::default())
                    .unwrap_or(taffy::NodeId::from(u64::MAX))
            });
        if let Err(e) = cx.taffy.set_node_context(node_id, Some(NodeContext::None)) {
            log::error!("Checkbox: set_node_context failed: {e}");
        }
        (LayoutId(node_id), CheckboxLayout { node_id })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        let id = cx.allocate_id::<Checkbox>();
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
                self.radius,
            );
        }
        cx.register_hit_region(HitRegion::new(id, bounds, 0).with_cursor(CursorStyle::Arrow));

        cx.prepaint_node_open(id, bounds, self.accessibility().expect("checkbox a11y"));
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
        let bx = self.box_bounds(bounds);
        let id = self.last_id;
        // Outer indicator: accent when checked, bordered surface when not. Hover/
        // press shade the fill (skipped while disabled).
        let outer = if self.disabled {
            self.muted
        } else if self.checked {
            id.map_or(self.accent, |id| interactive_fill(cx, id, self.accent))
        } else {
            self.border
        };
        push_rect(cx, bx, outer, self.radius);

        if self.checked {
            // Centred notch in the background colour reads as "filled / selected".
            let n = BOX_SIZE * 0.42;
            let notch = Bounds::from_origin_size(
                bx.origin.x + (BOX_SIZE - n) * 0.5,
                bx.origin.y + (BOX_SIZE - n) * 0.5,
                n,
                n,
            );
            push_rect(cx, notch, self.surface, self.radius * 0.5);
        } else {
            // Inner surface leaves the border showing as a 1.5px ring.
            let inset = 1.5;
            let inner = Bounds::from_origin_size(
                bx.origin.x + inset,
                bx.origin.y + inset,
                BOX_SIZE - 2.0 * inset,
                BOX_SIZE - 2.0 * inset,
            );
            push_rect(cx, inner, self.surface, (self.radius - inset).max(0.0));
        }

        if let Some(text) = self.text.as_mut()
            && let Some(child_bounds) =
                resolve_child_bounds(cx.taffy, layout_state.node_id, 0, bounds.origin)
        {
            text.paint(child_bounds, cx);
        }
    }

    fn accessibility(&self) -> Option<AccessibilityInfo> {
        Some(AccessibilityInfo {
            role: AccessibilityRole::Checkbox,
            label: Some(self.label.clone()),
            is_disabled: self.disabled,
            is_selected: Some(self.checked),
            ..Default::default()
        })
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for Checkbox {}

impl IntoElement for Checkbox {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
