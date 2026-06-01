//! Button — a labelled, clickable command control.
//!
//! `Button` is a leaf interactive element (not a styled `Div`) so it owns its
//! accessibility identity (`AccessibilityRole::Button`), its keyboard activation
//! (Enter / Space), and its interaction-state fill. The visible label or icon is
//! a single child element laid out and painted inside the button box; the
//! framework focus ring is painted by the focus system (no custom ring here).
//!
//! Colours come from the ambient [`theme`](crate::theme::theme): the resting
//! fill is the `accent` token, hover/press shade it (see
//! [`control_paint`](super::control_paint)), and disabled uses `muted`. Tokens
//! are resolved at construction under the render observer, so flipping the
//! theme mode re-renders the button with the new palette (Strategy A).

mod builder;

pub use builder::IconButton;

use std::sync::Arc;

use slate_renderer::Lpx;
use slate_renderer::scene::RectInstance;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{AnyElement, Element, IntoElement, Sealed};
use crate::event::{EventCtx, Handlers, KeyHandlers};
use crate::focus::FocusableEntry;
use crate::hit_test::{CursorStyle, HitRegion};
use crate::layout::resolve_child_bounds;
use crate::style::{Length, Style};
use crate::types::{
    AccessibilityInfo, AccessibilityRole, Bounds, Edges, ElementId, LayoutId, NodeContext,
};

use super::control_paint::interactive_fill;

/// Activation callback shared by click + keyboard (Enter / Space). Takes only
/// the [`EventCtx`] — a button press carries no meaningful per-event payload.
pub(crate) type Activate = Arc<dyn Fn(&mut EventCtx) + Send + Sync + 'static>;

/// A labelled, clickable command control.
///
/// # Example
///
/// ```ignore
/// Button::new("Save").on_click(move |_| save());
/// IconButton::new(Image::from_path("trash.png"), "Delete");
/// ```
pub struct Button {
    /// Visible content laid out + painted inside the button (a `Text` label or
    /// an icon element). `None` only in the degenerate empty-button case.
    pub(super) content: Option<AnyElement>,
    /// Accessible name. Text buttons mirror the visible label here; icon buttons
    /// require an explicit aria-label (the icon has no readable text).
    pub(super) a11y_label: String,
    /// Activation callback (click + Enter/Space). `None` => inert button.
    pub(super) on_activate: Option<Activate>,
    /// Non-interactive: registers no handlers/focus, paints the `muted` fill,
    /// and reports `is_disabled` to AccessKit.
    pub(super) disabled: bool,
    /// Resting fill (theme `accent`), resolved at build under the render observer.
    pub(super) fill: [f32; 4],
    /// Disabled fill (theme `muted`).
    pub(super) fill_disabled: [f32; 4],
    /// Corner radius in logical px (theme `radius.md`).
    pub(super) corner_radius: f32,
    /// Horizontal / vertical padding in logical px (theme spacing).
    pub(super) pad_x: f32,
    pub(super) pad_y: f32,
    /// Stable id allocated during prepaint (available afterwards).
    pub(super) last_id: Option<ElementId>,
}

/// Layout state for Button — stores its Taffy node id.
pub struct ButtonLayout {
    node_id: taffy::NodeId,
}

impl Button {
    /// Resolve the painted fill against disabled + interaction state. Disabled
    /// wins; otherwise hover/press shade the resting `accent` fill.
    fn resolved_fill(&self, cx: &PaintCtx) -> [f32; 4] {
        if self.disabled {
            return self.fill_disabled;
        }
        match self.last_id {
            Some(id) => interactive_fill(cx, id, self.fill),
            None => self.fill,
        }
    }
}

impl Element for Button {
    type LayoutState = ButtonLayout;
    type PaintState = ();

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        let mut children = Vec::new();
        if let Some(content) = self.content.as_mut() {
            children.push(content.request_layout(cx).0);
        }

        // Centre the label/icon with asymmetric padding (wider horizontally),
        // the conventional button shape.
        let style = Style::new()
            .align_items(taffy::style::AlignItems::Center)
            .justify_content(taffy::style::JustifyContent::Center)
            .padding(Edges::new(
                Length::Px(self.pad_y),
                Length::Px(self.pad_x),
                Length::Px(self.pad_y),
                Length::Px(self.pad_x),
            ));

        let node_id = cx
            .taffy
            .new_with_children(taffy::Style::from(&style), &children)
            .unwrap_or_else(|e| {
                log::error!("Button: new_with_children failed ({e}); rendering empty");
                cx.taffy
                    .new_leaf(taffy::Style::default())
                    .unwrap_or(taffy::NodeId::from(u64::MAX))
            });
        if let Err(e) = cx.taffy.set_node_context(node_id, Some(NodeContext::None)) {
            log::error!("Button: set_node_context failed: {e}");
        }

        (LayoutId(node_id), ButtonLayout { node_id })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        let id = cx.allocate_id::<Button>();
        self.last_id = Some(id);

        if !self.disabled
            && let Some(activate) = self.on_activate.clone()
        {
            let click = activate.clone();
            cx.register_handlers(
                id,
                Handlers {
                    on_click: Some(Arc::new(move |_ev, cx| {
                        click(cx);
                        cx.stop_propagation();
                    })),
                    ..Default::default()
                },
            );
            cx.register_key_handlers(
                id,
                KeyHandlers {
                    on_key_down: Some(builder::activation_key_handler(activate)),
                    ..Default::default()
                },
            );
        }
        if !self.disabled {
            cx.register_focusable(
                FocusableEntry {
                    id,
                    tab_index: 0,
                    focus_ring: true,
                },
                bounds,
                self.corner_radius,
            );
        }
        cx.register_hit_region(HitRegion::new(id, bounds, 0).with_cursor(CursorStyle::Arrow));

        // a11y: Button node owns the (presentational) label/icon child. Open
        // before recursing so the child nests, mirroring `Div`.
        cx.prepaint_node_open(id, bounds, self.accessibility().expect("button a11y"));
        cx.push_frame(id);
        if let Some(content) = self.content.as_mut()
            && let Some(child_bounds) =
                resolve_child_bounds(cx.taffy, layout_state.node_id, 0, bounds.origin)
        {
            content.prepaint(child_bounds, cx);
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
        let color = self.resolved_fill(cx);
        cx.scene.push_rect(RectInstance {
            rect: [
                Lpx(bounds.origin.x),
                Lpx(bounds.origin.y),
                Lpx(bounds.size.width),
                Lpx(bounds.size.height),
            ],
            color,
            corner_radius: Lpx(self.corner_radius),
            _pad: [0.0; 3],
        });

        if let Some(content) = self.content.as_mut()
            && let Some(child_bounds) =
                resolve_child_bounds(cx.taffy, layout_state.node_id, 0, bounds.origin)
        {
            content.paint(child_bounds, cx);
        }
    }

    fn accessibility(&self) -> Option<AccessibilityInfo> {
        Some(AccessibilityInfo {
            role: AccessibilityRole::Button,
            label: Some(self.a11y_label.clone()),
            is_disabled: self.disabled,
            ..Default::default()
        })
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl Sealed for Button {}

impl IntoElement for Button {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
