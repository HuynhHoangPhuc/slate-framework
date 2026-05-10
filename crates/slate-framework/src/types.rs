//! Core geometry and identity types for the Element system.

use std::sync::atomic::{AtomicU64, Ordering};

/// A point in 2D space (logical points).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A 2D size (logical points).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const ZERO: Self = Self {
        width: 0.0,
        height: 0.0,
    };

    pub const fn new(width: f32, height: f32) -> Self {
        Self { width, height }
    }
}

/// A rectangular region in logical points.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Bounds {
    pub origin: Point,
    pub size: Size,
}

impl Bounds {
    pub const ZERO: Self = Self {
        origin: Point::ZERO,
        size: Size::ZERO,
    };

    pub const fn new(origin: Point, size: Size) -> Self {
        Self { origin, size }
    }

    pub fn from_origin_size(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            origin: Point::new(x, y),
            size: Size::new(width, height),
        }
    }

    /// Returns true if the point is inside this bounds (inclusive).
    #[inline]
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.origin.x
            && point.x <= self.origin.x + self.size.width
            && point.y >= self.origin.y
            && point.y <= self.origin.y + self.size.height
    }

    /// Returns true if this bounds intersects another bounds.
    #[inline]
    pub fn intersects(&self, other: &Bounds) -> bool {
        self.origin.x < other.origin.x + other.size.width
            && self.origin.x + self.size.width > other.origin.x
            && self.origin.y < other.origin.y + other.size.height
            && self.origin.y + self.size.height > other.origin.y
    }

    /// Returns the intersection of two bounds, or None if they don't intersect.
    pub fn intersection(&self, other: &Bounds) -> Option<Self> {
        if !self.intersects(other) {
            return None;
        }
        let x = self.origin.x.max(other.origin.x);
        let y = self.origin.y.max(other.origin.y);
        let right = (self.origin.x + self.size.width).min(other.origin.x + other.size.width);
        let bottom = (self.origin.y + self.size.height).min(other.origin.y + other.size.height);
        Some(Self::from_origin_size(x, y, right - x, bottom - y))
    }
}

/// Edge insets (top, right, bottom, left) — used for padding/margin.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Edges<T: Copy + Default> {
    pub top: T,
    pub right: T,
    pub bottom: T,
    pub left: T,
}

impl<T: Copy + Default> Edges<T> {
    pub const fn new(top: T, right: T, bottom: T, left: T) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    pub const fn all(value: T) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    pub const fn symmetric(vertical: T, horizontal: T) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }
}

/// Opaque handle to a Taffy layout node.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LayoutId(pub taffy::NodeId);

impl From<taffy::NodeId> for LayoutId {
    fn from(id: taffy::NodeId) -> Self {
        Self(id)
    }
}

impl From<LayoutId> for taffy::NodeId {
    fn from(id: LayoutId) -> Self {
        id.0
    }
}

/// Global counter for unique element IDs within a session.
static ELEMENT_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Stable identity for an element across frames.
///
/// Used for persistent state (Phase 4), hit-testing (Phase 6),
/// and accessibility (Phase 5).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ElementId(pub u64);

impl ElementId {
    /// Generate the next unique ElementId (monotonic counter).
    ///
    /// Use for non-tree contexts (e.g., tests, internal bookkeeping).
    /// For stable cross-frame identity, prefer `PrepaintCtx::allocate_id`.
    pub fn next() -> Self {
        Self(ELEMENT_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// Create an ElementId from a hash value.
    ///
    /// Used by `PrepaintCtx::allocate_id` for tree-position keying.
    /// Same hash → same ElementId across frames.
    pub(crate) const fn from_hash(hash: u64) -> Self {
        Self(hash)
    }

    /// Root sentinel ElementId (hash 0).
    ///
    /// Used as the initial parent in `PrepaintCtx` before any elements are allocated.
    pub(crate) const fn root() -> Self {
        Self(0)
    }

    /// Create an ElementId with a specific value (for testing or serialization).
    pub const fn from_raw(id: u64) -> Self {
        Self(id)
    }

    /// Get the raw u64 value.
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Measure data attached to each Taffy node.
///
/// Used by `compute_layout_with_measure` to drive the measure closure.
#[derive(Clone, Debug, Default)]
pub enum NodeContext {
    /// Laid-out text — width/height are pre-shaped, no further measure needed.
    Text { width_lpx: f32, height_lpx: f32 },
    /// Image with intrinsic size.
    Image { width_lpx: f32, height_lpx: f32 },
    /// Container — Taffy computes from style + children.
    #[default]
    None,
}

/// Accessibility role for an element.
///
/// Standard ARIA roles for v1 widget set. Maps to accesskit::Role.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AccessibilityRole {
    #[default]
    Unknown,
    Button,
    Checkbox,
    Dialog,
    /// Container/grouping element (e.g., Div).
    Group,
    /// Heading with semantic level (1-6).
    Heading {
        level: u8,
    },
    Image,
    Label,
    Link,
    List,
    ListItem,
    Menu,
    MenuItem,
    ProgressBar,
    RadioButton,
    ScrollView,
    Slider,
    Switch,
    Tab,
    TabPanel,
    TextInput,
    Tooltip,
    Window,
    /// Element is not exposed to accessibility tree.
    None,
}

impl From<AccessibilityRole> for accesskit::Role {
    fn from(role: AccessibilityRole) -> Self {
        match role {
            AccessibilityRole::Unknown => accesskit::Role::Unknown,
            AccessibilityRole::Button => accesskit::Role::Button,
            AccessibilityRole::Checkbox => accesskit::Role::CheckBox,
            AccessibilityRole::Dialog => accesskit::Role::Dialog,
            AccessibilityRole::Group => accesskit::Role::Group,
            AccessibilityRole::Heading { .. } => accesskit::Role::Heading,
            AccessibilityRole::Image => accesskit::Role::Image,
            AccessibilityRole::Label => accesskit::Role::Label,
            AccessibilityRole::Link => accesskit::Role::Link,
            AccessibilityRole::List => accesskit::Role::List,
            AccessibilityRole::ListItem => accesskit::Role::ListItem,
            AccessibilityRole::Menu => accesskit::Role::Menu,
            AccessibilityRole::MenuItem => accesskit::Role::MenuItem,
            AccessibilityRole::ProgressBar => accesskit::Role::ProgressIndicator,
            AccessibilityRole::RadioButton => accesskit::Role::RadioButton,
            AccessibilityRole::ScrollView => accesskit::Role::ScrollView,
            AccessibilityRole::Slider => accesskit::Role::Slider,
            AccessibilityRole::Switch => accesskit::Role::Switch,
            AccessibilityRole::Tab => accesskit::Role::Tab,
            AccessibilityRole::TabPanel => accesskit::Role::TabPanel,
            AccessibilityRole::TextInput => accesskit::Role::TextInput,
            AccessibilityRole::Tooltip => accesskit::Role::Tooltip,
            AccessibilityRole::Window => accesskit::Role::Window,
            AccessibilityRole::None => accesskit::Role::Unknown,
        }
    }
}

/// Live region announcement priority for dynamic content.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LiveRegion {
    /// Polite — announced after current speech finishes.
    Polite,
    /// Assertive — interrupts current speech immediately.
    Assertive,
}

/// Accessibility info for an element.
///
/// Provides semantic information for screen readers and assistive technologies.
/// Elements return this via `Element::accessibility()`.
#[derive(Clone, Debug, Default)]
pub struct AccessibilityInfo {
    /// Semantic role of the element.
    pub role: AccessibilityRole,
    /// Human-readable label (e.g., button text, image alt text).
    pub label: Option<String>,
    /// Extended description for additional context.
    pub description: Option<String>,
    /// Current value (for sliders, text inputs, etc.).
    pub value: Option<String>,
    /// Whether the element is disabled (non-interactive).
    pub is_disabled: bool,
    /// Whether the element currently has keyboard focus.
    pub is_focused: bool,
    /// Whether a collapsible element is expanded (None if not collapsible).
    pub is_expanded: Option<bool>,
    /// Whether the element is selected (None if not selectable).
    pub is_selected: Option<bool>,
    /// Live region behavior for dynamic content updates.
    pub live_region: Option<LiveRegion>,
}

/// Accessibility node for the a11y tree.
///
/// Built during `prepaint` phase, consumed by platform accessibility backends.
#[derive(Clone, Debug)]
pub struct AccessibilityNode {
    /// Stable element identity.
    pub id: ElementId,
    /// Screen bounds in logical points.
    pub bounds: Bounds,
    /// Semantic accessibility information.
    pub info: AccessibilityInfo,
    /// Child nodes (for hierarchical tree structure).
    pub children: Vec<AccessibilityNode>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_contains() {
        let b = Bounds::from_origin_size(10.0, 10.0, 100.0, 50.0);
        assert!(b.contains(Point::new(50.0, 30.0)));
        assert!(b.contains(Point::new(10.0, 10.0))); // top-left edge
        assert!(b.contains(Point::new(110.0, 60.0))); // bottom-right edge
        assert!(!b.contains(Point::new(5.0, 30.0))); // left of bounds
        assert!(!b.contains(Point::new(115.0, 30.0))); // right of bounds
    }

    #[test]
    fn bounds_intersects() {
        let a = Bounds::from_origin_size(0.0, 0.0, 100.0, 100.0);
        let b = Bounds::from_origin_size(50.0, 50.0, 100.0, 100.0);
        let c = Bounds::from_origin_size(200.0, 200.0, 50.0, 50.0);

        assert!(a.intersects(&b));
        assert!(b.intersects(&a));
        assert!(!a.intersects(&c));
        assert!(!c.intersects(&a));
    }

    #[test]
    fn element_id_uniqueness() {
        let id1 = ElementId::next();
        let id2 = ElementId::next();
        let id3 = ElementId::next();
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert!(id2.0 > id1.0);
    }

    #[test]
    fn element_id_from_hash_stability() {
        // Same hash produces same ElementId across calls
        let id1 = ElementId::from_hash(12345);
        let id2 = ElementId::from_hash(12345);
        assert_eq!(id1, id2);

        // Different hashes produce different IDs
        let id3 = ElementId::from_hash(67890);
        assert_ne!(id1, id3);
    }

    #[test]
    fn element_id_root_sentinel() {
        let root = ElementId::root();
        assert_eq!(root.as_u64(), 0);

        // Root is distinct from next() IDs (which start at 1)
        let next_id = ElementId::next();
        assert_ne!(root, next_id);
    }
}
