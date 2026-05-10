//! Integration tests for Taffy layout computation and bounds resolution.

use slate_framework::layout::{LayoutTree, resolve_bounds};
use slate_framework::types::{LayoutId, NodeContext};
use taffy::prelude::*;

#[test]
fn layout_tree_new() {
    let tree = LayoutTree::new();
    drop(tree);
}

#[test]
fn layout_tree_create_leaf() {
    let mut tree = LayoutTree::new();
    let style = taffy::Style {
        size: taffy::Size {
            width: Dimension::length(100.0),
            height: Dimension::length(50.0),
        },
        ..Default::default()
    };
    let _node = tree.inner_mut().new_leaf(style).unwrap();
    // NodeId was created successfully
}

#[test]
fn compute_layout_basic() {
    let mut tree = LayoutTree::new();
    let style = taffy::Style {
        size: taffy::Size {
            width: Dimension::length(200.0),
            height: Dimension::length(100.0),
        },
        ..Default::default()
    };
    let node = tree.inner_mut().new_leaf(style).unwrap();

    tree.inner_mut()
        .compute_layout(
            node,
            taffy::Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        )
        .unwrap();

    let layout = tree.inner().layout(node).unwrap();
    assert_eq!(layout.size.width, 200.0);
    assert_eq!(layout.size.height, 100.0);
}

#[test]
fn resolve_bounds_basic() {
    let mut tree = LayoutTree::new();
    let style = taffy::Style {
        size: taffy::Size {
            width: Dimension::length(150.0),
            height: Dimension::length(75.0),
        },
        ..Default::default()
    };
    let node = tree.inner_mut().new_leaf(style).unwrap();

    tree.inner_mut()
        .compute_layout(
            node,
            taffy::Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        )
        .unwrap();

    let bounds = resolve_bounds(tree.inner(), LayoutId(node)).unwrap();
    assert_eq!(bounds.size.width, 150.0);
    assert_eq!(bounds.size.height, 75.0);
    assert_eq!(bounds.origin.x, 0.0);
    assert_eq!(bounds.origin.y, 0.0);
}

#[test]
fn nested_layout_child_offset() {
    let mut tree = LayoutTree::new();

    // Child leaf
    let child_style = taffy::Style {
        size: taffy::Size {
            width: Dimension::length(50.0),
            height: Dimension::length(50.0),
        },
        ..Default::default()
    };
    let child = tree.inner_mut().new_leaf(child_style).unwrap();

    // Parent with padding
    let parent_style = taffy::Style {
        size: taffy::Size {
            width: Dimension::length(200.0),
            height: Dimension::length(200.0),
        },
        padding: taffy::Rect {
            left: LengthPercentage::length(20.0),
            right: LengthPercentage::length(20.0),
            top: LengthPercentage::length(20.0),
            bottom: LengthPercentage::length(20.0),
        },
        ..Default::default()
    };
    let parent = tree
        .inner_mut()
        .new_with_children(parent_style, &[child])
        .unwrap();

    tree.inner_mut()
        .compute_layout(
            parent,
            taffy::Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        )
        .unwrap();

    let child_layout = tree.inner().layout(child).unwrap();
    // Child should be offset by parent's padding
    assert_eq!(child_layout.location.x, 20.0);
    assert_eq!(child_layout.location.y, 20.0);
}

#[test]
fn flex_row_children_layout() {
    let mut tree = LayoutTree::new();

    let child1 = tree
        .inner_mut()
        .new_leaf(taffy::Style {
            size: taffy::Size {
                width: Dimension::length(100.0),
                height: Dimension::length(50.0),
            },
            ..Default::default()
        })
        .unwrap();

    let child2 = tree
        .inner_mut()
        .new_leaf(taffy::Style {
            size: taffy::Size {
                width: Dimension::length(100.0),
                height: Dimension::length(50.0),
            },
            ..Default::default()
        })
        .unwrap();

    let parent = tree
        .inner_mut()
        .new_with_children(
            taffy::Style {
                display: Display::Flex,
                flex_direction: FlexDirection::Row,
                gap: taffy::Size {
                    width: LengthPercentage::length(10.0),
                    height: LengthPercentage::length(0.0),
                },
                ..Default::default()
            },
            &[child1, child2],
        )
        .unwrap();

    tree.inner_mut()
        .compute_layout(
            parent,
            taffy::Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        )
        .unwrap();

    let layout1 = tree.inner().layout(child1).unwrap();
    let layout2 = tree.inner().layout(child2).unwrap();

    // First child at x=0
    assert_eq!(layout1.location.x, 0.0);
    // Second child at x=100 (child1 width) + 10 (gap) = 110
    assert_eq!(layout2.location.x, 110.0);
}

#[test]
fn node_context_text() {
    let ctx = NodeContext::Text {
        width_lpx: 100.0,
        height_lpx: 20.0,
    };
    match ctx {
        NodeContext::Text {
            width_lpx,
            height_lpx,
        } => {
            assert_eq!(width_lpx, 100.0);
            assert_eq!(height_lpx, 20.0);
        }
        _ => panic!("expected Text variant"),
    }
}

#[test]
fn node_context_image() {
    let ctx = NodeContext::Image {
        width_lpx: 200.0,
        height_lpx: 150.0,
    };
    match ctx {
        NodeContext::Image {
            width_lpx,
            height_lpx,
        } => {
            assert_eq!(width_lpx, 200.0);
            assert_eq!(height_lpx, 150.0);
        }
        _ => panic!("expected Image variant"),
    }
}

#[test]
fn node_context_none_default() {
    let ctx = NodeContext::default();
    assert!(matches!(ctx, NodeContext::None));
}

#[test]
fn layout_viewport_is_logical_points() {
    // Verify that layout computation uses logical viewport dimensions.
    // A 200×100 child centered in an 800×600 viewport should be at:
    //   x = (800 - 200) / 2 = 300
    //   y = (600 - 100) / 2 = 250
    // This is independent of scale_factor (which only affects paint).
    let mut tree = LayoutTree::new();

    let child = tree
        .inner_mut()
        .new_leaf(taffy::Style {
            size: taffy::Size {
                width: Dimension::length(200.0),
                height: Dimension::length(100.0),
            },
            ..Default::default()
        })
        .unwrap();

    let parent = tree
        .inner_mut()
        .new_with_children(
            taffy::Style {
                display: Display::Flex,
                justify_content: Some(JustifyContent::Center),
                align_items: Some(AlignItems::Center),
                size: taffy::Size {
                    width: Dimension::length(800.0),
                    height: Dimension::length(600.0),
                },
                ..Default::default()
            },
            &[child],
        )
        .unwrap();

    // Simulate logical viewport 800×600 (as if scale_factor=2.0 with physical=1600×1200)
    tree.inner_mut()
        .compute_layout(
            parent,
            taffy::Size {
                width: AvailableSpace::Definite(800.0),
                height: AvailableSpace::Definite(600.0),
            },
        )
        .unwrap();

    let bounds = resolve_bounds(tree.inner(), LayoutId(child)).unwrap();
    // Child should be centered in logical coordinates
    assert_eq!(bounds.origin.x, 300.0); // (800-200)/2
    assert_eq!(bounds.origin.y, 250.0); // (600-100)/2
    assert_eq!(bounds.size.width, 200.0);
    assert_eq!(bounds.size.height, 100.0);
}
