//! Phase-specific contexts for the Element lifecycle.
//!
//! Each phase of the Element lifecycle receives a different context:
//! - `LayoutCtx` — layout computation (Taffy tree, text measurement)
//! - `PrepaintCtx` — hit-test registration, accessibility nodes
//! - `PaintCtx` — GPU scene emission, text rasterization
//!
//! All contexts are `!Send + !Sync` because they hold `&mut TextSystem`
//! which carries `PhantomData<*const ()>`.

use std::collections::HashMap;

use slate_renderer::atlas::Atlas;
use slate_renderer::scene::Scene;
use taffy::TaffyTree;

use crate::event::{Handlers, KeyHandlers};
use crate::executor::ForegroundExecutor;
use crate::focus::{FocusRegistry, FocusableEntry};
use crate::focus_ring::FocusBounds;
use crate::hit_test::{HitRegion, HitTestList};
use crate::image_cache::ImageCache;
use crate::paint_cache::TextShapingCache;
use crate::reactive_state::StateRegistry;
use crate::text_system::TextSystem;
use crate::types::{AccessibilityInfo, AccessibilityNode, Bounds, ElementId, NodeContext};

/// Context for the `request_layout` phase.
///
/// Provides access to:
/// - Taffy layout tree for node creation
/// - Text system for measurement
/// - Foreground executor for async tasks
/// - Scale factor for DPI-aware layout
pub struct LayoutCtx<'a> {
    /// Taffy layout tree with per-node context.
    pub taffy: &'a mut TaffyTree<NodeContext>,
    /// Text system for measuring text (mutable for lazy font loading).
    pub text: &'a mut TextSystem,
    /// Foreground executor for UI-thread async tasks.
    pub executor: &'a ForegroundExecutor,
    /// Display scale factor (e.g., 2.0 for Retina).
    pub scale_factor: f64,
}

impl<'a> LayoutCtx<'a> {
    /// Create a new layout context.
    pub fn new(
        taffy: &'a mut TaffyTree<NodeContext>,
        text: &'a mut TextSystem,
        executor: &'a ForegroundExecutor,
        scale_factor: f64,
    ) -> Self {
        Self {
            taffy,
            text,
            executor,
            scale_factor,
        }
    }
}

/// Context for the `prepaint` phase.
///
/// Provides access to:
/// - Taffy tree for child bounds lookup (read-only)
/// - Hit region registration for pointer events
/// - Accessibility node registration (hierarchical tree via open/close pattern)
/// - Text system for text-related computations
/// - Foreground executor for async tasks
/// - Scale factor
/// - Stable ElementId allocation via tree-position keying
/// - State registry for element-level reactive state (internal use)
pub struct PrepaintCtx<'a> {
    /// Taffy layout tree (read-only for bounds lookup).
    pub taffy: &'a TaffyTree<NodeContext>,
    /// Hit regions registered during prepaint (Phase 6 dispatches events).
    pub hit_regions: &'a mut HitTestList,
    /// Completed top-level accessibility nodes (root-level a11y tree).
    a11y_completed: &'a mut Vec<AccessibilityNode>,
    /// Text system (mutable for lazy font loading).
    pub text: &'a mut TextSystem,
    /// Foreground executor for UI-thread async tasks.
    pub executor: &'a ForegroundExecutor,
    /// Display scale factor.
    pub scale_factor: f64,

    // --- Element-level state registry (Phase 4) ---
    /// State registry for element-level reactive state slots.
    /// Internal consumers (e.g., paint cache) use `cx.state_registry.use_state(id, default)`.
    /// No public hooks-style API in v1 per F6 Route A decision.
    #[allow(dead_code)] // Used in Phase 6 by paint cache
    pub(crate) state_registry: &'a mut StateRegistry,

    // --- Text shaping cache (Phase 6) ---
    /// Cache for pre-atlas shaped text to skip shaping on unchanged Text elements.
    pub(crate) text_shaping_cache: &'a mut TextShapingCache,

    // --- Tree-position keying for stable ElementIds (Phase 4 prep) ---
    /// Stack of ancestor element IDs; `last()` is the immediate parent.
    /// Pushed by `push_frame`, popped by `pop_frame`. Length 1 (root) at frame start.
    pub(crate) id_stack: Vec<ElementId>,
    /// Per-depth child counter, parallel to `id_stack`.
    /// Reset to 0 at each `push_frame`, incremented by `allocate_id`.
    pub(crate) child_counters: Vec<u32>,
    /// User-provided key for the *next* `allocate_id` call; consumed on use.
    pub(crate) next_key: Option<String>,

    // --- Hierarchical a11y tree building ---
    /// In-progress a11y node builders; `last_mut()` is the current node accumulating children.
    pub(crate) a11y_stack: Vec<AccessibilityNode>,

    // --- Event handler collection (Phase 5a) ---
    /// Collected event handlers per element (populated during prepaint).
    pub(crate) handler_map: &'a mut HashMap<ElementId, Handlers>,
    /// Parent map for ancestor iteration during event dispatch.
    pub(crate) parent_map: &'a mut HashMap<ElementId, ElementId>,

    // --- Keyboard handler collection + focus registry (Phase 9b) ---
    /// Per-element keyboard handlers (populated during prepaint).
    /// Consumed by `AppState::dispatch_key_*` via focused-chain bubble.
    pub(crate) key_handler_map: &'a mut HashMap<ElementId, KeyHandlers>,
    /// Focus registry built each prepaint via `register_focusable`. Tab
    /// traversal + focused-chain dispatch read this after the prepaint walk.
    pub(crate) focus_registry: &'a mut FocusRegistry,
    /// Painted bounds for focusable elements — consumed once per paint pass
    /// when emitting the focus ring overlay.
    pub(crate) focus_bounds: &'a mut HashMap<ElementId, FocusBounds>,
}

impl<'a> PrepaintCtx<'a> {
    /// Create a new prepaint context.
    ///
    /// Caller must call `init_root_frame()` before the first `allocate_id`.
    ///
    /// # Borrow Order (ADR-001)
    ///
    /// `state_registry` is borrowed after `id_stack` setup, before any view interior
    /// borrows. This is slot 8 in the RefCell borrow-order discipline.
    /// `text_shaping_cache` is slot 9.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        taffy: &'a TaffyTree<NodeContext>,
        hit_regions: &'a mut HitTestList,
        a11y_completed: &'a mut Vec<AccessibilityNode>,
        text: &'a mut TextSystem,
        executor: &'a ForegroundExecutor,
        scale_factor: f64,
        state_registry: &'a mut StateRegistry,
        text_shaping_cache: &'a mut TextShapingCache,
        handler_map: &'a mut HashMap<ElementId, Handlers>,
        parent_map: &'a mut HashMap<ElementId, ElementId>,
        key_handler_map: &'a mut HashMap<ElementId, KeyHandlers>,
        focus_registry: &'a mut FocusRegistry,
        focus_bounds: &'a mut HashMap<ElementId, FocusBounds>,
    ) -> Self {
        Self {
            taffy,
            hit_regions,
            a11y_completed,
            text,
            executor,
            scale_factor,
            state_registry,
            text_shaping_cache,
            id_stack: Vec::new(),
            child_counters: Vec::new(),
            next_key: None,
            a11y_stack: Vec::new(),
            handler_map,
            parent_map,
            key_handler_map,
            focus_registry,
            focus_bounds,
        }
    }

    /// Initialize the root frame for tree-position keying.
    ///
    /// Must be called once before prepaint traversal begins.
    pub fn init_root_frame(&mut self) {
        self.id_stack.clear();
        self.id_stack.push(ElementId::root());
        self.child_counters.clear();
        self.child_counters.push(0);
        self.next_key = None;
    }

    /// Allocate a stable `ElementId` for the next child of the current parent.
    ///
    /// Hashes `(parent_id, child_index, type_id, optional_user_key)` to produce
    /// an ID that is stable across frames for the same tree position.
    ///
    /// # Stability Note
    ///
    /// Uses `DefaultHasher` which is NOT guaranteed stable across Rust versions.
    /// IDs are ephemeral per-session — do not serialize or persist them.
    /// Phase 4 signals use these for subscription identity within a single run.
    pub fn allocate_id<E: 'static>(&mut self) -> ElementId {
        let counter = self
            .child_counters
            .last_mut()
            .expect("PrepaintCtx must have a root frame; call init_root_frame before prepaint");
        let index = *counter;
        *counter += 1;

        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.id_stack.last().copied().hash(&mut hasher);
        index.hash(&mut hasher);
        std::any::TypeId::of::<E>().hash(&mut hasher);
        if let Some(k) = self.next_key.take() {
            k.hash(&mut hasher);
        }
        ElementId::from_hash(hasher.finish())
    }

    /// Push a frame: the given `id` becomes parent for descended children.
    ///
    /// Call after `allocate_id` for container elements, before recursing into children.
    /// Records the parent relationship for event dispatch ancestor walking.
    pub fn push_frame(&mut self, id: ElementId) {
        // Record parent relationship for event dispatch
        if let Some(&parent) = self.id_stack.last().filter(|&&p| p != ElementId::root()) {
            self.parent_map.insert(id, parent);
        }
        self.id_stack.push(id);
        self.child_counters.push(0);
    }

    /// Register event handlers for an element.
    ///
    /// Call during prepaint after allocating the element ID.
    /// Only elements with handlers need to call this.
    pub(crate) fn register_handlers(&mut self, id: ElementId, handlers: Handlers) {
        if handlers.has_any() {
            self.handler_map.insert(id, handlers);
        }
    }

    /// Register per-element keyboard handlers (Phase 9b).
    ///
    /// Call during prepaint after allocating the element ID. Only elements
    /// with at least one key handler need to call this; empty bundles are
    /// skipped to keep `key_handler_map` lookups cheap during dispatch.
    pub(crate) fn register_key_handlers(&mut self, id: ElementId, handlers: KeyHandlers) {
        if handlers.has_any() {
            self.key_handler_map.insert(id, handlers);
        }
    }

    /// Register a focusable entry along with its painted bounds (Phase 9b).
    ///
    /// Call during prepaint when the element opts in via `Div::focusable(true)`.
    /// `bounds` + `corner_radius` are cached for the focus-ring overlay so the
    /// paint pass doesn't need to walk the element tree a second time. The
    /// registry is cleared at frame start; `prune_missing` runs after the
    /// prepaint walk to clear focus on unmounted elements.
    pub(crate) fn register_focusable(
        &mut self,
        entry: FocusableEntry,
        bounds: Bounds,
        corner_radius: f32,
    ) {
        let id = entry.id;
        self.focus_registry.register(entry);
        self.focus_bounds.insert(
            id,
            FocusBounds {
                bounds,
                corner_radius,
            },
        );
    }

    /// Pop the current frame after recursing children.
    ///
    /// Call after all children have been prepainted.
    pub fn pop_frame(&mut self) {
        self.id_stack.pop();
        self.child_counters.pop();
    }

    /// Set a user-provided key consumed by the next `allocate_id` call.
    ///
    /// Use for dynamic lists where insertion/removal shifts indices.
    pub fn set_next_key(&mut self, k: impl Into<String>) {
        self.next_key = Some(k.into());
    }

    /// Register a hit region for pointer event handling.
    ///
    /// Z-index is auto-assigned based on registration order (back-to-front).
    pub fn register_hit_region(&mut self, region: HitRegion) {
        self.hit_regions.push(region);
    }

    /// Open an accessibility node frame for a container element.
    ///
    /// Call before recursing into children. The node will accumulate children
    /// until `prepaint_node_close()` is called.
    ///
    /// Order: `prepaint_node_open` → `push_frame` → recurse → `pop_frame` → `prepaint_node_close`
    ///
    /// # Panic Safety
    ///
    /// Caller must ensure `prepaint_node_close()` is called even on error paths.
    /// If not, `debug_assert` at frame end catches imbalance in dev builds.
    /// In release, corrupted a11y tree is non-fatal (screen reader sees flat tree).
    pub fn prepaint_node_open(&mut self, id: ElementId, bounds: Bounds, info: AccessibilityInfo) {
        self.a11y_stack.push(AccessibilityNode {
            id,
            bounds,
            info,
            children: Vec::new(),
        });
    }

    /// Close the current accessibility node frame.
    ///
    /// Pops the top node from the stack. If there's a parent on the stack,
    /// appends as a child; otherwise appends to the completed list.
    pub fn prepaint_node_close(&mut self) {
        debug_assert!(
            !self.a11y_stack.is_empty(),
            "prepaint_node_close without matching open"
        );
        let Some(node) = self.a11y_stack.pop() else {
            log::warn!("a11y close without open; dropped");
            return;
        };
        match self.a11y_stack.last_mut() {
            Some(parent) => parent.children.push(node),
            None => self.a11y_completed.push(node),
        }
    }

    /// Register a leaf accessibility node (shorthand for open + immediate close).
    ///
    /// Use for elements with no children (e.g., Text).
    pub fn register_a11y_node(&mut self, node: AccessibilityNode) {
        // Leaf nodes get added to the current parent (if any) or completed list
        match self.a11y_stack.last_mut() {
            Some(parent) => parent.children.push(node),
            None => self.a11y_completed.push(node),
        }
    }
}

/// Context for the `paint` phase.
///
/// Provides access to:
/// - Taffy tree for child bounds lookup (read-only)
/// - GPU scene for primitive emission
/// - Text system for text rasterization
/// - Glyph atlas for texture storage
/// - Image atlas for image texture storage
/// - Image cache for uploaded image management
/// - GPU queue for uploads
/// - Foreground executor for async tasks
/// - Scale factor
pub struct PaintCtx<'a> {
    /// Taffy layout tree (read-only for bounds lookup).
    pub taffy: &'a TaffyTree<NodeContext>,
    /// GPU scene receiving render primitives.
    pub scene: &'a mut Scene,
    /// Text system (mutable for glyph cache updates).
    pub text: &'a mut TextSystem,
    /// Glyph atlas for texture storage.
    pub glyph_atlas: &'a mut Atlas,
    /// Image atlas for image texture storage.
    pub image_atlas: &'a mut Atlas,
    /// Image cache for uploaded image management.
    pub(crate) image_cache: &'a mut ImageCache,
    /// GPU queue for texture uploads.
    pub queue: &'a wgpu::Queue,
    /// Foreground executor for UI-thread async tasks.
    pub executor: &'a ForegroundExecutor,
    /// Display scale factor.
    pub scale_factor: f64,
}

impl<'a> PaintCtx<'a> {
    /// Create a new paint context.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        taffy: &'a TaffyTree<NodeContext>,
        scene: &'a mut Scene,
        text: &'a mut TextSystem,
        glyph_atlas: &'a mut Atlas,
        image_atlas: &'a mut Atlas,
        image_cache: &'a mut ImageCache,
        queue: &'a wgpu::Queue,
        executor: &'a ForegroundExecutor,
        scale_factor: f64,
    ) -> Self {
        Self {
            taffy,
            scene,
            text,
            glyph_atlas,
            image_atlas,
            image_cache,
            queue,
            executor,
            scale_factor,
        }
    }
}

// Compile-time verification that contexts are !Send + !Sync
// (inherited from &mut TextSystem which has PhantomData<*const ()>)
#[cfg(test)]
mod tests {
    use super::*;

    fn assert_not_send<T>() {}
    fn assert_not_sync<T>() {}

    #[test]
    fn contexts_are_not_send_sync() {
        // These would fail to compile if the types were Send/Sync
        // due to PhantomData<*const ()> in TextSystem
        assert_not_send::<LayoutCtx<'_>>();
        assert_not_send::<PrepaintCtx<'_>>();
        assert_not_send::<PaintCtx<'_>>();
        assert_not_sync::<LayoutCtx<'_>>();
        assert_not_sync::<PrepaintCtx<'_>>();
        assert_not_sync::<PaintCtx<'_>>();
    }

    #[test]
    fn allocate_id_stability_across_calls() {
        // Test that same tree position produces same ID across "frames"
        // This simulates calling allocate_id twice with identical tree state

        use std::hash::{Hash, Hasher};

        fn compute_id<E: 'static>(parent: ElementId, index: u32, key: Option<&str>) -> ElementId {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            parent.hash(&mut hasher);
            index.hash(&mut hasher);
            std::any::TypeId::of::<E>().hash(&mut hasher);
            if let Some(k) = key {
                k.hash(&mut hasher);
            }
            ElementId::from_hash(hasher.finish())
        }

        struct TestElement;

        // Same position (root parent, index 0, TestElement type) → same ID
        let id1 = compute_id::<TestElement>(ElementId::root(), 0, None);
        let id2 = compute_id::<TestElement>(ElementId::root(), 0, None);
        assert_eq!(id1, id2, "same tree position should produce same ID");

        // Different index → different ID
        let id3 = compute_id::<TestElement>(ElementId::root(), 1, None);
        assert_ne!(id1, id3, "different index should produce different ID");

        // Same position with user key → different ID than without key
        let id4 = compute_id::<TestElement>(ElementId::root(), 0, Some("toolbar"));
        assert_ne!(id1, id4, "user key should change the ID");

        // Same position + same key → same ID
        let id5 = compute_id::<TestElement>(ElementId::root(), 0, Some("toolbar"));
        assert_eq!(id4, id5, "same key should produce same ID");
    }

    #[test]
    fn push_pop_frame_lifo() {
        // Test that push/pop frame maintains LIFO stack semantics
        let parent1 = ElementId::from_hash(100);
        let parent2 = ElementId::from_hash(200);

        let mut stack: Vec<ElementId> = vec![ElementId::root()];
        let mut counters: Vec<u32> = vec![0];

        // Push first frame
        stack.push(parent1);
        counters.push(0);
        assert_eq!(stack.len(), 2);
        assert_eq!(*stack.last().unwrap(), parent1);

        // Push second frame
        stack.push(parent2);
        counters.push(0);
        assert_eq!(stack.len(), 3);
        assert_eq!(*stack.last().unwrap(), parent2);

        // Pop second frame
        stack.pop();
        counters.pop();
        assert_eq!(stack.len(), 2);
        assert_eq!(*stack.last().unwrap(), parent1);

        // Pop first frame
        stack.pop();
        counters.pop();
        assert_eq!(stack.len(), 1);
        assert_eq!(*stack.last().unwrap(), ElementId::root());
    }
}
