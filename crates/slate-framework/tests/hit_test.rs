//! Integration tests for hit-testing infrastructure.

use slate_framework::hit_test::{CursorStyle, HitRegion, HitTestList};
use slate_framework::types::{Bounds, ElementId, Point};

#[test]
fn hit_test_list_new() {
    let list = HitTestList::new();
    assert!(list.hit_test(Point::new(0.0, 0.0)).is_none());
}

#[test]
fn hit_test_single_region() {
    let mut list = HitTestList::new();
    let bounds = Bounds::from_origin_size(10.0, 10.0, 100.0, 50.0);
    let region = HitRegion::new(ElementId::from_raw(1), bounds, 0);
    list.push(region);

    // Point inside
    let result = list.hit_test(Point::new(50.0, 30.0));
    assert!(result.is_some());
    assert_eq!(result.unwrap().element_id, ElementId::from_raw(1));

    // Point outside
    assert!(list.hit_test(Point::new(5.0, 5.0)).is_none());
    assert!(list.hit_test(Point::new(200.0, 200.0)).is_none());
}

#[test]
fn hit_test_z_order_topmost_wins() {
    let mut list = HitTestList::new();

    // Bottom region (pushed first, lower z)
    let bottom = HitRegion::new(
        ElementId::from_raw(1),
        Bounds::from_origin_size(0.0, 0.0, 100.0, 100.0),
        0,
    );
    list.push(bottom);

    // Top region (pushed second, higher z - overlaps bottom)
    let top = HitRegion::new(
        ElementId::from_raw(2),
        Bounds::from_origin_size(50.0, 50.0, 100.0, 100.0),
        0,
    );
    list.push(top);

    // Point in overlap area - top wins (higher z)
    let result = list.hit_test(Point::new(75.0, 75.0));
    assert!(result.is_some());
    assert_eq!(result.unwrap().element_id, ElementId::from_raw(2));

    // Point only in bottom
    let result = list.hit_test(Point::new(25.0, 25.0));
    assert!(result.is_some());
    assert_eq!(result.unwrap().element_id, ElementId::from_raw(1));
}

#[test]
fn hit_test_all_returns_stack() {
    let mut list = HitTestList::new();

    list.push(
        HitRegion::new(
            ElementId::from_raw(1),
            Bounds::from_origin_size(0.0, 0.0, 100.0, 100.0),
            0,
        )
        .with_opaque(false),
    );
    list.push(
        HitRegion::new(
            ElementId::from_raw(2),
            Bounds::from_origin_size(25.0, 25.0, 50.0, 50.0),
            0,
        )
        .with_opaque(false),
    );

    // Point in overlap - hit_test_all returns both
    let results = list.hit_test_all(Point::new(50.0, 50.0));
    assert_eq!(results.len(), 2);
    // Topmost first (higher z index)
    assert_eq!(results[0].element_id, ElementId::from_raw(2));
    assert_eq!(results[1].element_id, ElementId::from_raw(1));
}

#[test]
fn hit_test_clear() {
    let mut list = HitTestList::new();
    list.push(HitRegion::new(
        ElementId::from_raw(1),
        Bounds::from_origin_size(0.0, 0.0, 100.0, 100.0),
        0,
    ));

    assert!(list.hit_test(Point::new(50.0, 50.0)).is_some());

    list.clear();

    assert!(list.hit_test(Point::new(50.0, 50.0)).is_none());
}

#[test]
fn hit_region_cursor_style() {
    let region = HitRegion::new(
        ElementId::from_raw(1),
        Bounds::from_origin_size(0.0, 0.0, 100.0, 100.0),
        0,
    )
    .with_cursor(CursorStyle::Pointer);

    assert_eq!(region.cursor, CursorStyle::Pointer);
}

#[test]
fn hit_region_opaque_flag() {
    let region = HitRegion::new(
        ElementId::from_raw(1),
        Bounds::from_origin_size(0.0, 0.0, 100.0, 100.0),
        0,
    )
    .with_opaque(true);

    assert!(region.is_opaque);
}

#[test]
fn hit_test_all_stops_at_opaque() {
    let mut list = HitTestList::new();

    // Bottom region (non-opaque)
    list.push(
        HitRegion::new(
            ElementId::from_raw(1),
            Bounds::from_origin_size(0.0, 0.0, 100.0, 100.0),
            0,
        )
        .with_opaque(false),
    );

    // Middle opaque region
    list.push(
        HitRegion::new(
            ElementId::from_raw(2),
            Bounds::from_origin_size(25.0, 25.0, 50.0, 50.0),
            0,
        )
        .with_opaque(true),
    );

    // Top region (non-opaque)
    list.push(
        HitRegion::new(
            ElementId::from_raw(3),
            Bounds::from_origin_size(30.0, 30.0, 40.0, 40.0),
            0,
        )
        .with_opaque(false),
    );

    // hit_test_all stops at opaque middle, doesn't return bottom
    let results = list.hit_test_all(Point::new(50.0, 50.0));
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].element_id, ElementId::from_raw(3)); // top
    assert_eq!(results[1].element_id, ElementId::from_raw(2)); // middle (opaque)
    // bottom not included because middle is opaque
}

#[test]
fn cursor_style_default() {
    assert_eq!(CursorStyle::default(), CursorStyle::Arrow);
}
