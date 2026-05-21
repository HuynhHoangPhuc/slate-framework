//! Hit-test infrastructure for pointer event handling.
//!
//! Elements register rectangular hit regions during `prepaint`. Hit queries
//! walk the list back-to-front (painter's order) to find the topmost element
//! under a point.
//!
//! This is the production GPUI approach as of Feb 2026 — flat list iteration
//! over `SmallVec<HitRegion>`. The "BoundsTree" in GPUI is used for z-order
//! assignment during paint, not for hit-testing.

use smallvec::SmallVec;

use crate::types::{Bounds, ElementId, Point};

/// Mouse cursor style for hit regions.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum CursorStyle {
    /// Default arrow cursor.
    #[default]
    Arrow,
    /// Pointer/hand cursor (clickable elements).
    Pointer,
    /// Text selection cursor (I-beam).
    Text,
    /// Grab cursor (draggable elements).
    Grab,
    /// Grabbing cursor (currently dragging).
    Grabbing,
    /// Vertical resize cursor.
    ResizeNS,
    /// Horizontal resize cursor.
    ResizeEW,
    /// Diagonal resize (NW-SE).
    ResizeNWSE,
    /// Diagonal resize (NE-SW).
    ResizeNESW,
    /// Not allowed cursor.
    NotAllowed,
    /// Crosshair cursor.
    Crosshair,
    /// Move cursor.
    Move,
}

/// A rectangular region that can receive pointer events.
#[derive(Clone, Debug)]
pub struct HitRegion {
    /// Stable element identity.
    pub element_id: ElementId,
    /// Screen bounds in logical pixels.
    pub bounds: Bounds,
    /// Z-order index (higher = closer to viewer). Assigned by traversal order.
    pub z_index: u32,
    /// If true, this region blocks hits to regions behind it.
    pub is_opaque: bool,
    /// Cursor style to show when hovering this region.
    pub cursor: CursorStyle,
}

impl HitRegion {
    /// Create a new hit region with default cursor and opaque.
    pub fn new(element_id: ElementId, bounds: Bounds, z_index: u32) -> Self {
        Self {
            element_id,
            bounds,
            z_index,
            is_opaque: true,
            cursor: CursorStyle::Arrow,
        }
    }

    /// Set cursor style.
    pub fn with_cursor(mut self, cursor: CursorStyle) -> Self {
        self.cursor = cursor;
        self
    }

    /// Set opacity (whether this region blocks hits to regions behind).
    pub fn with_opaque(mut self, opaque: bool) -> Self {
        self.is_opaque = opaque;
        self
    }
}

/// Result of a hit test query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitTestResult {
    /// Element ID of the hit element.
    pub element_id: ElementId,
    /// Cursor style to show.
    pub cursor: CursorStyle,
}

/// Collection of hit regions for a frame.
///
/// Uses `SmallVec<[HitRegion; 32]>` for stack-inline storage on typical screens.
/// Spills to heap only on dense UI (>32 interactive regions visible).
#[derive(Clone, Debug, Default)]
pub struct HitTestList {
    regions: SmallVec<[HitRegion; 32]>,
    /// Current z-index counter (incremented as elements register).
    z_counter: u32,
}

impl HitTestList {
    /// Create a new empty hit test list.
    pub fn new() -> Self {
        Self {
            regions: SmallVec::new(),
            z_counter: 0,
        }
    }

    /// Clear all hit regions (call at start of each frame).
    pub fn clear(&mut self) {
        self.regions.clear();
        self.z_counter = 0;
    }

    /// Register a hit region with auto-assigned z-index.
    pub fn push(&mut self, mut region: HitRegion) {
        region.z_index = self.z_counter;
        self.z_counter += 1;
        self.regions.push(region);
    }

    /// Number of registered hit regions.
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    /// Returns true if no hit regions are registered.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    /// Returns true if `id` has a registered hit region this frame. Used to
    /// detect unmounted elements (e.g. to auto-release a stale mouse capture).
    pub fn contains(&self, id: ElementId) -> bool {
        self.regions.iter().any(|r| r.element_id == id)
    }

    /// Find the topmost element under a point.
    ///
    /// Walks the list back-to-front (last registered = topmost).
    /// Returns `None` if no element is under the point.
    pub fn hit_test(&self, point: Point) -> Option<HitTestResult> {
        self.regions
            .iter()
            .rev()
            .find(|r| r.bounds.contains(point))
            .map(|r| HitTestResult {
                element_id: r.element_id,
                cursor: r.cursor,
            })
    }

    /// Find all elements under a point, topmost first.
    ///
    /// If an opaque region is found, regions behind it are excluded.
    pub fn hit_test_all(&self, point: Point) -> Vec<HitTestResult> {
        let mut results = Vec::new();
        for region in self.regions.iter().rev() {
            if region.bounds.contains(point) {
                results.push(HitTestResult {
                    element_id: region.element_id,
                    cursor: region.cursor,
                });
                if region.is_opaque {
                    break;
                }
            }
        }
        results
    }

    /// Get the cursor style for the topmost element under a point.
    pub fn cursor_at(&self, point: Point) -> CursorStyle {
        self.hit_test(point)
            .map(|r| r.cursor)
            .unwrap_or(CursorStyle::Arrow)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(id: u64, x: f32, y: f32, w: f32, h: f32) -> HitRegion {
        HitRegion::new(
            ElementId::from_raw(id),
            Bounds::from_origin_size(x, y, w, h),
            0,
        )
    }

    #[test]
    fn hit_test_empty_list() {
        let list = HitTestList::new();
        assert!(list.hit_test(Point::new(50.0, 50.0)).is_none());
        assert!(list.hit_test_all(Point::new(50.0, 50.0)).is_empty());
    }

    #[test]
    fn hit_test_single_region() {
        let mut list = HitTestList::new();
        list.push(region(1, 0.0, 0.0, 100.0, 100.0));

        let result = list.hit_test(Point::new(50.0, 50.0));
        assert!(result.is_some());
        assert_eq!(result.unwrap().element_id, ElementId::from_raw(1));

        // Miss
        assert!(list.hit_test(Point::new(150.0, 50.0)).is_none());
    }

    #[test]
    fn hit_test_overlapping_topmost_wins() {
        let mut list = HitTestList::new();
        // Bottom layer (registered first)
        list.push(region(1, 0.0, 0.0, 100.0, 100.0));
        // Top layer (registered second, overlaps)
        list.push(region(2, 25.0, 25.0, 50.0, 50.0));

        // Point inside both — topmost (region 2) wins
        let result = list.hit_test(Point::new(50.0, 50.0));
        assert_eq!(result.unwrap().element_id, ElementId::from_raw(2));

        // Point only in bottom region
        let result = list.hit_test(Point::new(10.0, 10.0));
        assert_eq!(result.unwrap().element_id, ElementId::from_raw(1));
    }

    #[test]
    fn hit_test_all_returns_all_under_point() {
        let mut list = HitTestList::new();
        list.push(region(1, 0.0, 0.0, 100.0, 100.0).with_opaque(false));
        list.push(region(2, 25.0, 25.0, 50.0, 50.0).with_opaque(false));

        let results = list.hit_test_all(Point::new(50.0, 50.0));
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].element_id, ElementId::from_raw(2)); // topmost first
        assert_eq!(results[1].element_id, ElementId::from_raw(1));
    }

    #[test]
    fn hit_test_all_stops_at_opaque() {
        let mut list = HitTestList::new();
        list.push(region(1, 0.0, 0.0, 100.0, 100.0));
        list.push(region(2, 25.0, 25.0, 50.0, 50.0).with_opaque(true)); // opaque

        let results = list.hit_test_all(Point::new(50.0, 50.0));
        assert_eq!(results.len(), 1); // only topmost, stopped by opaque
        assert_eq!(results[0].element_id, ElementId::from_raw(2));
    }

    #[test]
    fn hit_test_cursor_style() {
        let mut list = HitTestList::new();
        list.push(region(1, 0.0, 0.0, 100.0, 100.0).with_cursor(CursorStyle::Pointer));

        let result = list.hit_test(Point::new(50.0, 50.0)).unwrap();
        assert_eq!(result.cursor, CursorStyle::Pointer);
    }

    #[test]
    fn hit_test_z_order_auto_assigned() {
        let mut list = HitTestList::new();
        list.push(region(1, 0.0, 0.0, 100.0, 100.0));
        list.push(region(2, 0.0, 0.0, 100.0, 100.0));
        list.push(region(3, 0.0, 0.0, 100.0, 100.0));

        // z_index should be auto-assigned 0, 1, 2
        assert_eq!(list.regions[0].z_index, 0);
        assert_eq!(list.regions[1].z_index, 1);
        assert_eq!(list.regions[2].z_index, 2);
    }

    #[test]
    fn hit_test_clear_resets() {
        let mut list = HitTestList::new();
        list.push(region(1, 0.0, 0.0, 100.0, 100.0));
        assert!(!list.is_empty());

        list.clear();
        assert!(list.is_empty());
        assert_eq!(list.len(), 0);
    }
}
