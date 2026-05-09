//! Div — flexbox container element.
//!
//! Div is the primary container element, supporting:
//! - Flexbox layout via Taffy (using unified Style type)
//! - Background color with corner radius
//! - Padding and margin
//! - Child elements

use std::sync::Arc;

use slate_renderer::scene::RectInstance;
use taffy::prelude::*;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{AnyElement, Element, IntoElement, Sealed};
use crate::event::{
    EventCtx, Handlers, MouseEvent, MouseHandler, PointerEvent, PointerHandler, ScrollEvent,
    ScrollHandler,
};
use crate::hit_test::{CursorStyle, HitRegion};
use crate::layout::resolve_child_bounds;
use crate::style::Style;
use crate::types::{
    AccessibilityInfo, AccessibilityRole, Bounds, ElementId, LayoutId, NodeContext,
};

/// Flexbox container element.
///
/// # Example
///
/// ```ignore
/// Div::new()
///     .background([0.2, 0.2, 0.2, 1.0])
///     .corner_radius(8.0)
///     .style(|s| s.padding_all(16.0).gap(8.0))
///     .child(Text::new("Hello"))
///     .child(Text::new("World"))
/// ```
pub struct Div {
    children: Vec<AnyElement>,
    layout_style: Style,
    visual: DivVisual,
    /// User-provided stability key for dynamic lists (consumed during prepaint).
    user_key: Option<String>,
    /// Stable ElementId allocated during prepaint (available after prepaint).
    last_id: Option<ElementId>,
    // -------------------------------------------------------------------------
    // Event handlers (Phase 5a)
    // -------------------------------------------------------------------------
    /// Handler for synthesized click events (down+up on same target).
    pub(crate) on_click: Option<MouseHandler>,
    /// Handler for mouse button down events.
    pub(crate) on_mouse_down: Option<MouseHandler>,
    /// Handler for mouse button up events.
    pub(crate) on_mouse_up: Option<MouseHandler>,
    /// Handler for mouse move events (coalesced, one per frame).
    pub(crate) on_mouse_move: Option<MouseHandler>,
    /// Handler for scroll wheel/trackpad events.
    pub(crate) on_mouse_scrolled: Option<ScrollHandler>,
    /// Handler for raw pointer events (no coalescing).
    pub(crate) on_pointer_event: Option<PointerHandler>,
    /// Handler for pointer enter events.
    pub(crate) on_pointer_enter: Option<PointerHandler>,
    /// Handler for pointer leave events.
    pub(crate) on_pointer_leave: Option<PointerHandler>,
}

/// Visual styling for a Div (non-layout properties).
#[derive(Clone, Debug, Default)]
pub struct DivVisual {
    /// Background color (linear, premultiplied RGBA).
    pub background: Option<[f32; 4]>,
    /// Corner radius in logical pixels.
    pub corner_radius: f32,
}

/// Layout state for Div — stores Taffy node ID.
pub struct DivLayoutState {
    node_id: taffy::NodeId,
}

/// Paint state for Div — currently empty.
pub struct DivPaintState;

impl Div {
    /// Create a new empty Div.
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            layout_style: Style::default(),
            visual: DivVisual::default(),
            user_key: None,
            last_id: None,
            on_click: None,
            on_mouse_down: None,
            on_mouse_up: None,
            on_mouse_move: None,
            on_mouse_scrolled: None,
            on_pointer_event: None,
            on_pointer_enter: None,
            on_pointer_leave: None,
        }
    }

    /// Set a stability key for dynamic lists.
    ///
    /// Use when child order or count changes between frames (e.g., list items).
    /// Static trees can omit — tree-position keying handles them automatically.
    pub fn key(mut self, k: impl Into<String>) -> Self {
        self.user_key = Some(k.into());
        self
    }

    /// Add a child element.
    pub fn child<E: IntoElement>(mut self, child: E) -> Self {
        self.children.push(AnyElement::new(child));
        self
    }

    /// Add a type-erased child element.
    ///
    /// Use this when you already have an `AnyElement` (e.g., from a builder).
    pub fn child_any(mut self, child: AnyElement) -> Self {
        self.children.push(child);
        self
    }

    /// Add multiple children.
    pub fn children<I, E>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = E>,
        E: IntoElement,
    {
        for child in children {
            self.children.push(AnyElement::new(child));
        }
        self
    }

    /// Configure layout style via closure.
    ///
    /// # Example
    /// ```ignore
    /// Div::new().style(|s| s.padding_all(16.0).gap(8.0).column())
    /// ```
    pub fn style(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        self.layout_style = f(self.layout_style);
        self
    }

    /// Set background color (linear, premultiplied RGBA).
    pub fn background(mut self, color: [f32; 4]) -> Self {
        self.visual.background = Some(color);
        self
    }

    /// Set corner radius in logical pixels.
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.visual.corner_radius = radius;
        self
    }

    /// Set flex direction (convenience method).
    pub fn direction(mut self, direction: FlexDirection) -> Self {
        self.layout_style.flex_direction = direction;
        self
    }

    /// Set flex direction to column (convenience method).
    pub fn column(mut self) -> Self {
        self.layout_style.flex_direction = FlexDirection::Column;
        self
    }

    /// Set flex direction to row (convenience method).
    pub fn row(mut self) -> Self {
        self.layout_style.flex_direction = FlexDirection::Row;
        self
    }

    /// Set flex grow (convenience method).
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.layout_style.flex_grow = grow;
        self
    }

    /// Set gap between children (convenience method).
    pub fn gap(mut self, gap: f32) -> Self {
        self.layout_style.gap = gap;
        self
    }

    // -------------------------------------------------------------------------
    // Event handler builders (Phase 5a)
    // -------------------------------------------------------------------------

    /// Register a click handler (fires when mouse down+up lands on same target).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let count = Signal::new(cx.runtime(), 0);
    /// Div::new().on_click(move |_, _| count.set(count.get() + 1))
    /// ```
    pub fn on_click<F>(mut self, handler: F) -> Self
    where
        F: Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_click = Some(Arc::new(handler));
        self
    }

    /// Register a mouse down handler.
    ///
    /// # Example
    ///
    /// ```ignore
    /// Div::new().on_mouse_down(|event, ctx| {
    ///     println!("Down at {:?}", event.position);
    ///     ctx.stop_propagation();
    /// })
    /// ```
    pub fn on_mouse_down<F>(mut self, handler: F) -> Self
    where
        F: Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_mouse_down = Some(Arc::new(handler));
        self
    }

    /// Register a mouse up handler.
    pub fn on_mouse_up<F>(mut self, handler: F) -> Self
    where
        F: Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_mouse_up = Some(Arc::new(handler));
        self
    }

    /// Register a mouse move handler (coalesced, one call per frame).
    pub fn on_mouse_move<F>(mut self, handler: F) -> Self
    where
        F: Fn(&MouseEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_mouse_move = Some(Arc::new(handler));
        self
    }

    /// Register a scroll wheel/trackpad handler.
    ///
    /// # Example
    ///
    /// ```ignore
    /// Div::new().on_mouse_scrolled(|event, _| {
    ///     let scale = if event.precise { 1.0 } else { 12.0 };
    ///     scroll_offset.set(scroll_offset.get() + event.delta_y * scale);
    /// })
    /// ```
    pub fn on_mouse_scrolled<F>(mut self, handler: F) -> Self
    where
        F: Fn(&ScrollEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_mouse_scrolled = Some(Arc::new(handler));
        self
    }

    /// Register a raw pointer event handler (no coalescing).
    ///
    /// Receives all pointer events (down, up, move, enter, leave) in order.
    pub fn on_pointer_event<F>(mut self, handler: F) -> Self
    where
        F: Fn(&PointerEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_pointer_event = Some(Arc::new(handler));
        self
    }

    /// Register a pointer enter handler.
    pub fn on_pointer_enter<F>(mut self, handler: F) -> Self
    where
        F: Fn(&PointerEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_pointer_enter = Some(Arc::new(handler));
        self
    }

    /// Register a pointer leave handler.
    pub fn on_pointer_leave<F>(mut self, handler: F) -> Self
    where
        F: Fn(&PointerEvent, &mut EventCtx) + Send + Sync + 'static,
    {
        self.on_pointer_leave = Some(Arc::new(handler));
        self
    }
}

impl Default for Div {
    fn default() -> Self {
        Self::new()
    }
}

impl Sealed for Div {}

impl Element for Div {
    type LayoutState = DivLayoutState;
    type PaintState = DivPaintState;

    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
        // Layout children first, collect their node IDs
        let mut child_nodes = Vec::with_capacity(self.children.len());
        for child in &mut self.children {
            let layout_id = child.request_layout(cx);
            child_nodes.push(layout_id.0);
        }

        // Convert unified Style to taffy::Style
        let taffy_style = taffy::Style::from(&self.layout_style);
        let node_id = match cx.taffy.new_with_children(taffy_style, &child_nodes) {
            Ok(id) => id,
            Err(e) => {
                log::error!("Div: failed to create Taffy node: {e}; rendering empty");
                // Sentinel: empty leaf node so layout completes; widget renders zero-size
                match cx.taffy.new_leaf(taffy::Style::default()) {
                    Ok(id) => id,
                    Err(e2) => {
                        log::error!("Div: Taffy new_leaf also failed ({e2}) — pathological state");
                        taffy::NodeId::from(u64::MAX)
                    }
                }
            }
        };

        // Set node context (container) — non-fatal if fails
        if let Err(e) = cx.taffy.set_node_context(node_id, Some(NodeContext::None)) {
            log::error!("Div: failed to set node context: {e}; layout proceeds without context");
        }

        (LayoutId(node_id), DivLayoutState { node_id })
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        // Tree-position keying: set user key if provided, allocate stable ID
        if let Some(k) = self.user_key.take() {
            cx.set_next_key(k);
        }
        let element_id = cx.allocate_id::<Div>();
        self.last_id = Some(element_id);

        // Register event handlers for dispatch (Phase 5a)
        cx.register_handlers(
            element_id,
            Handlers {
                on_click: self.on_click.clone(),
                on_mouse_down: self.on_mouse_down.clone(),
                on_mouse_up: self.on_mouse_up.clone(),
                on_mouse_move: self.on_mouse_move.clone(),
                on_mouse_scrolled: self.on_mouse_scrolled.clone(),
                on_pointer_event: self.on_pointer_event.clone(),
                on_pointer_enter: self.on_pointer_enter.clone(),
                on_pointer_leave: self.on_pointer_leave.clone(),
            },
        );

        // Register hit region if Div has background (clickable container)
        if self.visual.background.is_some() {
            cx.register_hit_region(
                HitRegion::new(element_id, bounds, 0).with_cursor(CursorStyle::Arrow),
            );
        }

        // Open a11y node before children (will accumulate children's a11y nodes)
        // Order: prepaint_node_open → push_frame → recurse → pop_frame → prepaint_node_close
        //
        // PANIC SAFETY: If child.prepaint() panics, a11y_stack will be left unbalanced.
        // - Debug builds: frame-end debug_assert catches this before render
        // - Release builds: corrupted a11y tree is non-fatal (screen reader sees flat tree)
        // - Child prepaint implementations should not panic in normal operation
        let opened_a11y = if let Some(info) = self.accessibility() {
            cx.prepaint_node_open(element_id, bounds, info);
            true
        } else {
            false
        };

        // Push frame so children's IDs derive from this Div's ID
        cx.push_frame(element_id);

        // Prepaint children with their resolved bounds
        for (i, child) in self.children.iter_mut().enumerate() {
            if let Some(child_bounds) =
                resolve_child_bounds(cx.taffy, layout_state.node_id, i, bounds.origin)
            {
                child.prepaint(child_bounds, cx);
            }
        }

        // Pop frame after children are processed
        cx.pop_frame();

        // Close a11y node after children (children are now nested)
        if opened_a11y {
            cx.prepaint_node_close();
        }

        DivPaintState
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        _paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    ) {
        // Paint background if set
        if let Some(color) = self.visual.background {
            let scale = cx.scale_factor as f32;
            cx.scene.push_rect(RectInstance {
                rect: [
                    bounds.origin.x * scale,
                    bounds.origin.y * scale,
                    bounds.size.width * scale,
                    bounds.size.height * scale,
                ],
                color,
                corner_radius: self.visual.corner_radius * scale,
                _pad: [0.0; 3],
            });
        }

        // Paint children
        for (i, child) in self.children.iter_mut().enumerate() {
            if let Some(child_bounds) =
                resolve_child_bounds(cx.taffy, layout_state.node_id, i, bounds.origin)
            {
                child.paint(child_bounds, cx);
            }
        }
    }

    fn accessibility(&self) -> Option<AccessibilityInfo> {
        Some(AccessibilityInfo {
            role: AccessibilityRole::Group,
            ..Default::default()
        })
    }

    fn id(&self) -> Option<ElementId> {
        self.last_id
    }
}

impl IntoElement for Div {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
