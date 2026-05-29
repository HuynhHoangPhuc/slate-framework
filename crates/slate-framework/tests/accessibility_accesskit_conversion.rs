//! Round-trip test for `slate_framework::a11y_accesskit::to_accesskit_node`.
//!
//! Builds an `AccessibilityNode` tree by hand (no rendering — keeps the test
//! focused on the conversion contract) and asserts that the produced
//! `accesskit::Node` exposes the same fields via accesskit's own getters.

use accesskit::{Action as AkAction, Live, NodeId, Role};
use slate_framework::a11y_accesskit::{
    element_id_to_node_id, to_accesskit_node, to_accesskit_tree,
};
use slate_framework::types::{
    AccessibilityAction, AccessibilityInfo, AccessibilityNode, AccessibilityRelationships,
    AccessibilityRole, Bounds, ElementId, LiveRegion,
};

fn make_node(
    id: ElementId,
    role: AccessibilityRole,
    label: Option<&str>,
    children: Vec<AccessibilityNode>,
    actions: Vec<AccessibilityAction>,
) -> AccessibilityNode {
    AccessibilityNode {
        id,
        bounds: Bounds::from_origin_size(10.0, 20.0, 100.0, 40.0),
        info: AccessibilityInfo {
            role,
            label: label.map(String::from),
            ..Default::default()
        },
        children,
        actions,
    }
}

#[test]
fn role_label_and_bounds_round_trip() {
    let node = make_node(
        ElementId::from_raw(7),
        AccessibilityRole::Button,
        Some("Submit"),
        Vec::new(),
        vec![AccessibilityAction::Click, AccessibilityAction::Focus],
    );

    let ak = to_accesskit_node(&node);

    assert_eq!(ak.role(), Role::Button);
    assert_eq!(ak.label(), Some("Submit"));

    let rect = ak.bounds().expect("bounds set");
    assert!((rect.x0 - 10.0).abs() < 1e-6);
    assert!((rect.y0 - 20.0).abs() < 1e-6);
    assert!((rect.x1 - 110.0).abs() < 1e-6);
    assert!((rect.y1 - 60.0).abs() < 1e-6);

    assert!(ak.supports_action(AkAction::Click));
    assert!(ak.supports_action(AkAction::Focus));
    assert!(!ak.supports_action(AkAction::Blur));
}

#[test]
fn states_relationships_and_live_region_round_trip() {
    let label_id = ElementId::from_raw(101);
    let desc_id = ElementId::from_raw(102);
    let controlled_id = ElementId::from_raw(103);
    let owned_id = ElementId::from_raw(104);

    let mut info = AccessibilityInfo {
        role: AccessibilityRole::Checkbox,
        label: Some("Agree".into()),
        description: Some("End-user licence".into()),
        is_disabled: true,
        is_expanded: Some(true),
        is_selected: Some(false),
        live_region: Some(LiveRegion::Assertive),
        tab_index: Some(3),
        ..Default::default()
    };
    info.relationships = AccessibilityRelationships {
        labelled_by: vec![label_id],
        described_by: vec![desc_id],
        controls: vec![controlled_id],
        owns: vec![owned_id],
    };

    let node = AccessibilityNode {
        id: ElementId::from_raw(1),
        bounds: Bounds::from_origin_size(0.0, 0.0, 10.0, 10.0),
        info,
        children: Vec::new(),
        actions: vec![AccessibilityAction::Expand, AccessibilityAction::Collapse],
    };

    let ak = to_accesskit_node(&node);

    assert_eq!(ak.role(), Role::CheckBox);
    assert!(ak.is_disabled());
    assert_eq!(ak.is_expanded(), Some(true));
    assert_eq!(ak.is_selected(), Some(false));
    assert_eq!(ak.live(), Some(Live::Assertive));

    assert_eq!(ak.labelled_by(), &[element_id_to_node_id(label_id)]);
    assert_eq!(ak.described_by(), &[element_id_to_node_id(desc_id)]);
    assert_eq!(ak.controls(), &[element_id_to_node_id(controlled_id)]);
    assert_eq!(ak.owns(), &[element_id_to_node_id(owned_id)]);

    assert!(ak.supports_action(AkAction::Expand));
    assert!(ak.supports_action(AkAction::Collapse));
}

#[test]
fn tree_flattens_depth_first_with_child_ids() {
    let leaf_a = make_node(
        ElementId::from_raw(10),
        AccessibilityRole::Label,
        Some("A"),
        Vec::new(),
        Vec::new(),
    );
    let leaf_b = make_node(
        ElementId::from_raw(11),
        AccessibilityRole::Label,
        Some("B"),
        Vec::new(),
        Vec::new(),
    );
    let root = make_node(
        ElementId::from_raw(1),
        AccessibilityRole::Group,
        None,
        vec![leaf_a, leaf_b],
        Vec::new(),
    );

    let flat = to_accesskit_tree(&root);
    assert_eq!(flat.len(), 3, "root + two leaves");

    assert_eq!(flat[0].0, NodeId(1));
    assert_eq!(flat[1].0, NodeId(10));
    assert_eq!(flat[2].0, NodeId(11));

    let root_ak = &flat[0].1;
    assert_eq!(root_ak.role(), Role::Group);
    assert_eq!(root_ak.children(), &[NodeId(10), NodeId(11)]);

    assert_eq!(flat[1].1.label(), Some("A"));
    assert_eq!(flat[2].1.label(), Some("B"));
}

#[test]
fn accessibility_action_to_accesskit_action_full_mapping() {
    assert_eq!(AkAction::from(AccessibilityAction::Click), AkAction::Click);
    assert_eq!(AkAction::from(AccessibilityAction::Focus), AkAction::Focus);
    assert_eq!(AkAction::from(AccessibilityAction::Blur), AkAction::Blur);
    assert_eq!(
        AkAction::from(AccessibilityAction::Increment),
        AkAction::Increment
    );
    assert_eq!(
        AkAction::from(AccessibilityAction::Decrement),
        AkAction::Decrement
    );
    assert_eq!(
        AkAction::from(AccessibilityAction::Expand),
        AkAction::Expand
    );
    assert_eq!(
        AkAction::from(AccessibilityAction::Collapse),
        AkAction::Collapse
    );
    assert_eq!(
        AkAction::from(AccessibilityAction::ScrollIntoView),
        AkAction::ScrollIntoView
    );
}
