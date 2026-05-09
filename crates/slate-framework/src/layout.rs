//! Layout computation and tree management.
//!
//! Provides `LayoutTree` as a wrapper around `TaffyTree<NodeContext>` and
//! layout computation functions for the Element lifecycle.

use taffy::prelude::*;

use crate::context::LayoutCtx;
use crate::element::AnyElement;
use crate::types::{Bounds, LayoutId, NodeContext, Point, Size};

/// Wrapper around Taffy's layout tree.
///
/// Manages the lifecycle of Taffy nodes and provides a clean API
/// for layout computation.
pub struct LayoutTree {
    inner: TaffyTree<NodeContext>,
}

impl LayoutTree {
    /// Create a new empty layout tree.
    pub fn new() -> Self {
        Self {
            inner: TaffyTree::new(),
        }
    }

    /// Get mutable access to the inner TaffyTree.
    pub fn inner_mut(&mut self) -> &mut TaffyTree<NodeContext> {
        &mut self.inner
    }

    /// Get shared access to the inner TaffyTree.
    pub fn inner(&self) -> &TaffyTree<NodeContext> {
        &self.inner
    }

    /// Clear all nodes from the tree.
    ///
    /// Call this at the start of each frame to reset layout state.
    /// The underlying allocation is preserved for reuse.
    pub fn clear(&mut self) {
        self.inner.clear();
    }

    /// Create a new leaf node with the given style.
    pub fn new_leaf(&mut self, style: taffy::Style) -> Result<LayoutId, taffy::TaffyError> {
        self.inner.new_leaf(style).map(LayoutId)
    }

    /// Create a new leaf node with context data.
    pub fn new_leaf_with_context(
        &mut self,
        style: taffy::Style,
        context: NodeContext,
    ) -> Result<LayoutId, taffy::TaffyError> {
        self.inner
            .new_leaf_with_context(style, context)
            .map(LayoutId)
    }

    /// Create a new container node with children.
    pub fn new_with_children(
        &mut self,
        style: taffy::Style,
        children: &[taffy::NodeId],
    ) -> Result<LayoutId, taffy::TaffyError> {
        self.inner.new_with_children(style, children).map(LayoutId)
    }

    /// Set node context.
    pub fn set_node_context(
        &mut self,
        node: LayoutId,
        context: Option<NodeContext>,
    ) -> Result<(), taffy::TaffyError> {
        self.inner.set_node_context(node.0, context)
    }

    /// Get the computed layout for a node.
    pub fn layout(&self, node: LayoutId) -> Result<&taffy::Layout, taffy::TaffyError> {
        self.inner.layout(node.0)
    }

    /// Get children of a node.
    pub fn children(&self, node: LayoutId) -> Result<Vec<taffy::NodeId>, taffy::TaffyError> {
        self.inner.children(node.0)
    }
}

impl Default for LayoutTree {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute layout for an element tree.
///
/// Calls `request_layout` on the root element to build the Taffy tree,
/// then runs Taffy's layout algorithm with the given available space.
///
/// # Arguments
///
/// * `root` - The root element to layout
/// * `cx` - Layout context with Taffy tree access
/// * `available_space` - Available space for layout (typically window size)
///
/// # Returns
///
/// The root node's `LayoutId` after layout computation, or `None` on failure.
pub fn compute_layout(
    root: &mut AnyElement,
    cx: &mut LayoutCtx,
    available_space: Size,
) -> Option<LayoutId> {
    let root_id = root.request_layout(cx);

    cx.taffy
        .compute_layout_with_measure(
            root_id.0,
            taffy::Size {
                width: AvailableSpace::Definite(available_space.width),
                height: AvailableSpace::Definite(available_space.height),
            },
            |_known, _available, _node_id, ctx, _style| match ctx {
                Some(NodeContext::Text {
                    width_lpx,
                    height_lpx,
                })
                | Some(NodeContext::Image {
                    width_lpx,
                    height_lpx,
                }) => taffy::Size {
                    width: *width_lpx,
                    height: *height_lpx,
                },
                _ => taffy::Size::ZERO,
            },
        )
        .ok()?;

    Some(root_id)
}

/// Resolve pixel bounds for a layout node.
///
/// Extracts the computed position and size from the Taffy layout result.
///
/// # Arguments
///
/// * `taffy` - The Taffy tree containing computed layouts
/// * `node` - The node to get bounds for
///
/// # Returns
///
/// The computed `Bounds` in physical pixels, or `None` if node not found.
pub fn resolve_bounds(taffy: &TaffyTree<NodeContext>, node: LayoutId) -> Option<Bounds> {
    let layout = taffy.layout(node.0).ok()?;

    Some(Bounds {
        origin: Point {
            x: layout.location.x,
            y: layout.location.y,
        },
        size: Size {
            width: layout.size.width,
            height: layout.size.height,
        },
    })
}

/// Resolve bounds for a child node relative to parent origin.
///
/// # Arguments
///
/// * `taffy` - The Taffy tree containing computed layouts
/// * `parent_id` - The parent node ID
/// * `child_index` - Index of the child within the parent
/// * `parent_origin` - Absolute position of the parent
///
/// # Returns
///
/// The child's absolute `Bounds`, or `None` if child not found.
pub fn resolve_child_bounds(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_tree_basic() {
        let mut tree = LayoutTree::new();

        let leaf = tree
            .new_leaf_with_context(
                taffy::Style {
                    size: taffy::Size {
                        width: Dimension::length(100.0),
                        height: Dimension::length(50.0),
                    },
                    ..Default::default()
                },
                NodeContext::None,
            )
            .unwrap();

        let root = tree
            .new_with_children(
                taffy::Style {
                    padding: Rect {
                        left: LengthPercentage::length(10.0),
                        right: LengthPercentage::length(10.0),
                        top: LengthPercentage::length(10.0),
                        bottom: LengthPercentage::length(10.0),
                    },
                    ..Default::default()
                },
                &[leaf.0],
            )
            .unwrap();

        tree.inner_mut()
            .compute_layout(
                root.0,
                taffy::Size {
                    width: AvailableSpace::Definite(500.0),
                    height: AvailableSpace::Definite(500.0),
                },
            )
            .unwrap();

        let root_layout = tree.layout(root).unwrap();
        assert_eq!(root_layout.size.width, 120.0); // 100 + 10 + 10 padding
        assert_eq!(root_layout.size.height, 70.0); // 50 + 10 + 10 padding

        let leaf_layout = tree.layout(leaf).unwrap();
        assert_eq!(leaf_layout.location.x, 10.0); // left padding
        assert_eq!(leaf_layout.location.y, 10.0); // top padding
    }

    #[test]
    fn resolve_bounds_basic() {
        let mut tree = TaffyTree::<NodeContext>::new();

        let node = tree
            .new_leaf(taffy::Style {
                size: taffy::Size {
                    width: Dimension::length(200.0),
                    height: Dimension::length(100.0),
                },
                ..Default::default()
            })
            .unwrap();

        tree.compute_layout(
            node,
            taffy::Size {
                width: AvailableSpace::Definite(500.0),
                height: AvailableSpace::Definite(500.0),
            },
        )
        .unwrap();

        let bounds = resolve_bounds(&tree, LayoutId(node)).unwrap();
        assert_eq!(bounds.origin.x, 0.0);
        assert_eq!(bounds.origin.y, 0.0);
        assert_eq!(bounds.size.width, 200.0);
        assert_eq!(bounds.size.height, 100.0);
    }

    #[test]
    fn layout_tree_clear() {
        let mut tree = LayoutTree::new();

        let _node = tree.new_leaf(taffy::Style::default()).unwrap();

        tree.clear();

        // After clear, creating a new node should work
        let node2 = tree.new_leaf(taffy::Style::default());
        assert!(node2.is_ok());
    }

    #[test]
    fn nested_div_text_layout() {
        // Simulates: Div { padding: 10, gap: 4, children: [Text("hello"), Text("world")] }
        // Text sizes are mocked as if shaped text returned these dimensions.
        let mut tree = TaffyTree::<NodeContext>::new();

        // Simulate Text("hello") - 40x14 px
        let text1 = tree
            .new_leaf_with_context(
                taffy::Style {
                    size: taffy::Size {
                        width: Dimension::length(40.0),
                        height: Dimension::length(14.0),
                    },
                    ..Default::default()
                },
                NodeContext::Text {
                    width_lpx: 40.0,
                    height_lpx: 14.0,
                },
            )
            .unwrap();

        // Simulate Text("world") - 45x14 px
        let text2 = tree
            .new_leaf_with_context(
                taffy::Style {
                    size: taffy::Size {
                        width: Dimension::length(45.0),
                        height: Dimension::length(14.0),
                    },
                    ..Default::default()
                },
                NodeContext::Text {
                    width_lpx: 45.0,
                    height_lpx: 14.0,
                },
            )
            .unwrap();

        // Div with padding: 10, gap: 4, direction: row (default)
        let div = tree
            .new_with_children(
                taffy::Style {
                    display: Display::Flex,
                    flex_direction: FlexDirection::Row,
                    padding: Rect {
                        left: LengthPercentage::length(10.0),
                        right: LengthPercentage::length(10.0),
                        top: LengthPercentage::length(10.0),
                        bottom: LengthPercentage::length(10.0),
                    },
                    gap: taffy::Size {
                        width: LengthPercentage::length(4.0),
                        height: LengthPercentage::length(0.0),
                    },
                    ..Default::default()
                },
                &[text1, text2],
            )
            .unwrap();

        // Compute layout in available space
        tree.compute_layout(
            div,
            taffy::Size {
                width: AvailableSpace::Definite(500.0),
                height: AvailableSpace::Definite(500.0),
            },
        )
        .unwrap();

        // Verify Div bounds
        // Width = 10 (left) + 40 (text1) + 4 (gap) + 45 (text2) + 10 (right) = 109
        // Height = 10 (top) + 14 (max child height) + 10 (bottom) = 34
        let div_bounds = resolve_bounds(&tree, LayoutId(div)).unwrap();
        assert_eq!(div_bounds.origin.x, 0.0);
        assert_eq!(div_bounds.origin.y, 0.0);
        assert_eq!(div_bounds.size.width, 109.0);
        assert_eq!(div_bounds.size.height, 34.0);

        // Verify text1 bounds (relative to parent origin 0,0)
        let text1_bounds = resolve_child_bounds(&tree, div, 0, Point::ZERO).unwrap();
        assert_eq!(text1_bounds.origin.x, 10.0); // left padding
        assert_eq!(text1_bounds.origin.y, 10.0); // top padding
        assert_eq!(text1_bounds.size.width, 40.0);
        assert_eq!(text1_bounds.size.height, 14.0);

        // Verify text2 bounds
        let text2_bounds = resolve_child_bounds(&tree, div, 1, Point::ZERO).unwrap();
        assert_eq!(text2_bounds.origin.x, 54.0); // 10 (pad) + 40 (text1) + 4 (gap)
        assert_eq!(text2_bounds.origin.y, 10.0); // top padding
        assert_eq!(text2_bounds.size.width, 45.0);
        assert_eq!(text2_bounds.size.height, 14.0);
    }

    #[test]
    fn resolve_child_bounds_with_offset() {
        let mut tree = TaffyTree::<NodeContext>::new();

        let child = tree
            .new_leaf(taffy::Style {
                size: taffy::Size {
                    width: Dimension::length(50.0),
                    height: Dimension::length(30.0),
                },
                ..Default::default()
            })
            .unwrap();

        let parent = tree
            .new_with_children(
                taffy::Style {
                    padding: Rect {
                        left: LengthPercentage::length(5.0),
                        right: LengthPercentage::length(5.0),
                        top: LengthPercentage::length(5.0),
                        bottom: LengthPercentage::length(5.0),
                    },
                    ..Default::default()
                },
                &[child],
            )
            .unwrap();

        tree.compute_layout(
            parent,
            taffy::Size {
                width: AvailableSpace::Definite(500.0),
                height: AvailableSpace::Definite(500.0),
            },
        )
        .unwrap();

        // Test with parent at offset (100, 200)
        let parent_origin = Point::new(100.0, 200.0);
        let child_bounds = resolve_child_bounds(&tree, parent, 0, parent_origin).unwrap();

        assert_eq!(child_bounds.origin.x, 105.0); // 100 + 5 padding
        assert_eq!(child_bounds.origin.y, 205.0); // 200 + 5 padding
        assert_eq!(child_bounds.size.width, 50.0);
        assert_eq!(child_bounds.size.height, 30.0);
    }
}
