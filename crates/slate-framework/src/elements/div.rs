//! Div — flexbox container element.
//!
//! Div is the primary container element, supporting:
//! - Flexbox layout via Taffy
//! - Background color with corner radius
//! - Padding and margin
//! - Child elements

use slate_renderer::scene::RectInstance;
use taffy::prelude::*;

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::element::{AnyElement, Element, IntoElement, Sealed};
use crate::types::{Bounds, Edges, LayoutId, NodeContext, Point, Size};

/// Flexbox container element.
///
/// # Example
///
/// ```ignore
/// Div::new()
///     .background([0.2, 0.2, 0.2, 1.0])
///     .padding(Edges::all(16.0))
///     .corner_radius(8.0)
///     .child(Text::new("Hello"))
///     .child(Text::new("World"))
/// ```
pub struct Div {
    children: Vec<AnyElement>,
    style: DivStyle,
}

/// Visual styling for a Div.
#[derive(Clone, Debug)]
pub struct DivStyle {
    /// Background color (linear, premultiplied RGBA).
    pub background: Option<[f32; 4]>,
    /// Corner radius in logical pixels.
    pub corner_radius: f32,
    /// Padding inside the container.
    pub padding: Edges<f32>,
    /// Margin outside the container.
    pub margin: Edges<f32>,
    /// Gap between children.
    pub gap: f32,
    /// Flex direction.
    pub direction: FlexDirection,
    /// Align items on cross axis.
    pub align_items: AlignItems,
    /// Justify content on main axis.
    pub justify_content: JustifyContent,
    /// Flex grow factor.
    pub flex_grow: f32,
    /// Flex shrink factor.
    pub flex_shrink: f32,
    /// Fixed width (optional).
    pub width: Option<f32>,
    /// Fixed height (optional).
    pub height: Option<f32>,
}

impl Default for DivStyle {
    fn default() -> Self {
        Self {
            background: None,
            corner_radius: 0.0,
            padding: Edges::default(),
            margin: Edges::default(),
            gap: 0.0,
            direction: FlexDirection::Row,
            align_items: AlignItems::Stretch,
            justify_content: JustifyContent::FlexStart,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            width: None,
            height: None,
        }
    }
}

/// Layout state for Div — stores child layout IDs and resolved bounds.
pub struct DivLayoutState {
    /// This Div's Taffy node ID.
    node_id: taffy::NodeId,
    /// Child element count (reserved for future use).
    #[allow(dead_code)]
    child_count: usize,
}

/// Paint state for Div — currently empty.
pub struct DivPaintState;

impl Div {
    /// Create a new empty Div.
    pub fn new() -> Self {
        Self {
            children: Vec::new(),
            style: DivStyle::default(),
        }
    }

    /// Add a child element.
    pub fn child<E: IntoElement>(mut self, child: E) -> Self {
        self.children.push(AnyElement::new(child));
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

    /// Set background color (linear, premultiplied RGBA).
    pub fn background(mut self, color: [f32; 4]) -> Self {
        self.style.background = Some(color);
        self
    }

    /// Set corner radius in logical pixels.
    pub fn corner_radius(mut self, radius: f32) -> Self {
        self.style.corner_radius = radius;
        self
    }

    /// Set padding.
    pub fn padding(mut self, padding: Edges<f32>) -> Self {
        self.style.padding = padding;
        self
    }

    /// Set margin.
    pub fn margin(mut self, margin: Edges<f32>) -> Self {
        self.style.margin = margin;
        self
    }

    /// Set gap between children.
    pub fn gap(mut self, gap: f32) -> Self {
        self.style.gap = gap;
        self
    }

    /// Set flex direction.
    pub fn direction(mut self, direction: FlexDirection) -> Self {
        self.style.direction = direction;
        self
    }

    /// Set align items.
    pub fn align_items(mut self, align: AlignItems) -> Self {
        self.style.align_items = align;
        self
    }

    /// Set justify content.
    pub fn justify_content(mut self, justify: JustifyContent) -> Self {
        self.style.justify_content = justify;
        self
    }

    /// Set flex grow.
    pub fn flex_grow(mut self, grow: f32) -> Self {
        self.style.flex_grow = grow;
        self
    }

    /// Set flex shrink.
    pub fn flex_shrink(mut self, shrink: f32) -> Self {
        self.style.flex_shrink = shrink;
        self
    }

    /// Set fixed width.
    pub fn width(mut self, width: f32) -> Self {
        self.style.width = Some(width);
        self
    }

    /// Set fixed height.
    pub fn height(mut self, height: f32) -> Self {
        self.style.height = Some(height);
        self
    }

    /// Build a Taffy style from DivStyle.
    fn build_taffy_style(&self) -> Style {
        Style {
            display: Display::Flex,
            flex_direction: self.style.direction,
            align_items: Some(self.style.align_items),
            justify_content: Some(self.style.justify_content),
            flex_grow: self.style.flex_grow,
            flex_shrink: self.style.flex_shrink,
            padding: Rect {
                left: LengthPercentage::length(self.style.padding.left),
                right: LengthPercentage::length(self.style.padding.right),
                top: LengthPercentage::length(self.style.padding.top),
                bottom: LengthPercentage::length(self.style.padding.bottom),
            },
            margin: Rect {
                left: LengthPercentageAuto::length(self.style.margin.left),
                right: LengthPercentageAuto::length(self.style.margin.right),
                top: LengthPercentageAuto::length(self.style.margin.top),
                bottom: LengthPercentageAuto::length(self.style.margin.bottom),
            },
            gap: taffy::Size {
                width: LengthPercentage::length(self.style.gap),
                height: LengthPercentage::length(self.style.gap),
            },
            size: taffy::Size {
                width: self
                    .style
                    .width
                    .map(Dimension::length)
                    .unwrap_or(Dimension::auto()),
                height: self
                    .style
                    .height
                    .map(Dimension::length)
                    .unwrap_or(Dimension::auto()),
            },
            ..Default::default()
        }
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

        // Create this Div's Taffy node with children
        let style = self.build_taffy_style();
        let node_id = cx
            .taffy
            .new_with_children(style, &child_nodes)
            .expect("failed to create Div node");

        // Set node context
        cx.taffy
            .set_node_context(node_id, Some(NodeContext::None))
            .expect("failed to set node context");

        let state = DivLayoutState {
            node_id,
            child_count: self.children.len(),
        };

        (LayoutId(node_id), state)
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState {
        // Prepaint children with their resolved bounds
        for (i, child) in self.children.iter_mut().enumerate() {
            if let Some(child_bounds) =
                get_child_bounds(cx.taffy, layout_state.node_id, i, bounds.origin)
            {
                child.prepaint(child_bounds, cx);
            }
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
        if let Some(color) = self.style.background {
            let scale = cx.scale_factor as f32;
            cx.scene.push_rect(RectInstance {
                rect: [
                    bounds.origin.x * scale,
                    bounds.origin.y * scale,
                    bounds.size.width * scale,
                    bounds.size.height * scale,
                ],
                color,
                corner_radius: self.style.corner_radius * scale,
                _pad: [0.0; 3],
            });
        }

        // Paint children
        for (i, child) in self.children.iter_mut().enumerate() {
            if let Some(child_bounds) =
                get_child_bounds(cx.taffy, layout_state.node_id, i, bounds.origin)
            {
                child.paint(child_bounds, cx);
            }
        }
    }
}

impl IntoElement for Div {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

/// Helper to get child bounds from Taffy layout.
fn get_child_bounds(
    taffy: &TaffyTree<NodeContext>,
    parent_id: taffy::NodeId,
    child_index: usize,
    parent_origin: Point,
) -> Option<Bounds> {
    let children = taffy.children(parent_id).ok()?;
    let child_id = children.get(child_index)?;
    let layout = taffy.layout(*child_id).ok()?;

    Some(Bounds {
        origin: Point {
            x: parent_origin.x + layout.location.x,
            y: parent_origin.y + layout.location.y,
        },
        size: Size {
            width: layout.size.width,
            height: layout.size.height,
        },
    })
}

