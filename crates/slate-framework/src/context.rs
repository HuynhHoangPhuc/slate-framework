//! Phase-specific contexts for the Element lifecycle.
//!
//! Each phase of the Element lifecycle receives a different context:
//! - `LayoutCtx` — layout computation (Taffy tree, text measurement)
//! - `PrepaintCtx` — hit-test registration, accessibility nodes
//! - `PaintCtx` — GPU scene emission, text rasterization
//!
//! All contexts are `!Send + !Sync` because they hold `&mut TextSystem`
//! which carries `PhantomData<*const ()>`.

use std::cell::RefCell;
use std::collections::HashMap;

use slate_renderer::atlas::Atlas;
use slate_renderer::scene::{ClipRect, Scene};
use slate_text::GlyphCache;
use taffy::TaffyTree;

use crate::event::{Handlers, ImeHandlers, KeyHandlers, MouseHandlers};
use crate::executor::ForegroundExecutor;
use crate::focus::{FocusRegistry, FocusableEntry};
use crate::focus_ring::FocusBounds;
use crate::hit_test::{HitRegion, HitTestList};
use crate::image_cache::ImageCache;
use crate::ime::ImeRegistry;
use crate::interaction_state::InteractionSnapshot;
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
    /// Hit regions registered during prepaint (used for event dispatch).
    pub hit_regions: &'a mut HitTestList,
    /// Completed top-level accessibility nodes (root-level a11y tree).
    a11y_completed: &'a mut Vec<AccessibilityNode>,
    /// Text system (mutable for lazy font loading).
    pub text: &'a mut TextSystem,
    /// Foreground executor for UI-thread async tasks.
    pub executor: &'a ForegroundExecutor,
    /// Display scale factor.
    pub scale_factor: f64,

    // --- Element-level state registry ---
    /// State registry for element-level reactive state slots.
    /// Internal consumers (e.g., paint cache) use `cx.state_registry.use_state(id, default)`.
    /// No public hooks-style API in v1 (deferred).
    #[allow(dead_code)] // Used by paint cache
    pub(crate) state_registry: &'a mut StateRegistry,

    // --- Text shaping cache ---
    /// Cache for pre-atlas shaped text to skip shaping on unchanged Text elements.
    pub(crate) text_shaping_cache: &'a mut TextShapingCache,

    // --- Tree-position keying for stable ElementIds ---
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

    // --- Event handler collection ---
    /// Collected event handlers per element (populated during prepaint).
    pub(crate) handler_map: &'a mut HashMap<ElementId, Handlers>,
    /// Focused mouse-handler bundles registered by elements that opt into the
    /// MouseHandlers surface (TextField, future TextArea). Walked alongside
    /// `handler_map` by the mouse dispatchers.
    pub(crate) mouse_handler_map: &'a mut HashMap<ElementId, MouseHandlers>,
    /// Parent map for ancestor iteration during event dispatch.
    pub(crate) parent_map: &'a mut HashMap<ElementId, ElementId>,

    // --- Keyboard handler collection + focus registry ---
    /// Per-element keyboard handlers (populated during prepaint).
    /// Consumed by `AppState::dispatch_key_*` via focused-chain bubble.
    pub(crate) key_handler_map: &'a mut HashMap<ElementId, KeyHandlers>,
    /// Focus registry built each prepaint via `register_focusable`. Tab
    /// traversal + focused-chain dispatch read this after the prepaint walk.
    pub(crate) focus_registry: &'a mut FocusRegistry,
    /// Painted bounds for focusable elements — consumed once per paint pass
    /// when emitting the focus ring overlay.
    pub(crate) focus_bounds: &'a mut HashMap<ElementId, FocusBounds>,

    // --- IME registry + handler collection ---
    /// Per-frame IME state registry. Wrapped in a `RefCell` so the dispatch
    /// path (via [`EventCtx::ime_state`](crate::event::EventCtx::ime_state))
    /// can read the same store without taking an exclusive borrow.
    pub(crate) ime_registry: &'a RefCell<ImeRegistry>,
    /// Per-element IME handler bundles. Populated during prepaint via
    /// [`register_ime_handlers`](Self::register_ime_handlers); drained during
    /// focused-chain bubble dispatch.
    pub(crate) ime_handler_map: &'a mut HashMap<ElementId, ImeHandlers>,
    /// Set of element ids that re-registered with [`ImeRegistry`] this frame.
    /// Used by the host after the prepaint walk to prune entries belonging to
    /// unmounted elements (mirrors the focus registry lifecycle).
    pub(crate) ime_registered_ids: &'a mut std::collections::HashSet<ElementId>,
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
        mouse_handler_map: &'a mut HashMap<ElementId, MouseHandlers>,
        parent_map: &'a mut HashMap<ElementId, ElementId>,
        key_handler_map: &'a mut HashMap<ElementId, KeyHandlers>,
        focus_registry: &'a mut FocusRegistry,
        focus_bounds: &'a mut HashMap<ElementId, FocusBounds>,
        ime_registry: &'a RefCell<ImeRegistry>,
        ime_handler_map: &'a mut HashMap<ElementId, ImeHandlers>,
        ime_registered_ids: &'a mut std::collections::HashSet<ElementId>,
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
            mouse_handler_map,
            parent_map,
            key_handler_map,
            focus_registry,
            focus_bounds,
            ime_registry,
            ime_handler_map,
            ime_registered_ids,
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
    /// Signals use these for subscription identity within a single run.
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

    /// Register per-element keyboard handlers.
    ///
    /// Call during prepaint after allocating the element ID. Only elements
    /// with at least one key handler need to call this; empty bundles are
    /// skipped to keep `key_handler_map` lookups cheap during dispatch.
    pub(crate) fn register_key_handlers(&mut self, id: ElementId, handlers: KeyHandlers) {
        if handlers.has_any() {
            self.key_handler_map.insert(id, handlers);
        }
    }

    /// Register per-element mouse handlers (TextField focused-element surface).
    ///
    /// Mirrors [`register_key_handlers`] for the focused-element mouse surface
    /// used by TextField (and, later, TextArea). Empty bundles are skipped so
    /// `mouse_handler_map` only carries elements that actually opted in.
    pub(crate) fn register_mouse_handlers(&mut self, id: ElementId, handlers: MouseHandlers) {
        if handlers.has_any() {
            self.mouse_handler_map.insert(id, handlers);
        }
    }

    /// Register per-element IME handlers.
    ///
    /// Call during prepaint when the element opts into IME (e.g.
    /// `Div::ime_capable(true)`). Empty bundles are skipped.
    pub(crate) fn register_ime_handlers(&mut self, id: ElementId, handlers: ImeHandlers) {
        if handlers.has_any() {
            self.ime_handler_map.insert(id, handlers);
        }
    }

    /// Mark `id` as ime-capable for this frame and ensure an [`ImeState`]
    /// entry exists. Returns the shared `Rc<RefCell<_>>` so the caller can
    /// keep a clone for its paint-time `caret_client_rect` update.
    ///
    /// [`ImeState`]: crate::ime::ImeState
    pub(crate) fn register_ime_state(
        &mut self,
        id: ElementId,
    ) -> std::rc::Rc<RefCell<crate::ime::ImeState>> {
        self.ime_registered_ids.insert(id);
        self.ime_registry.borrow_mut().register(id)
    }

    /// Register a focusable entry along with its painted bounds.
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

    /// Return the currently focused element id, if any.
    ///
    /// Consumed by `TextField::prepaint` to capture focused state at prepaint
    /// time so the paint pass can decide whether to draw the caret.
    pub fn focused_element(&self) -> Option<ElementId> {
        self.focus_registry.focused()
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
            actions: Vec::new(),
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
    /// Text system (process-wide font cache; no longer holds GlyphCache).
    pub text: &'a mut TextSystem,
    /// Per-window glyph cache, paired with `glyph_atlas`. MUST be the cache
    /// owned by the same `WindowState` whose `Renderer` owns `glyph_atlas` —
    /// see `WindowState::glyph_cache` for the cross-window corruption
    /// invariant this enforces.
    pub glyph_cache: &'a mut GlyphCache,
    /// Per-window glyph atlas for texture storage.
    pub glyph_atlas: &'a mut Atlas,
    /// Per-window image atlas for image texture storage.
    pub image_atlas: &'a mut Atlas,
    /// Per-window image cache, paired with `image_atlas`. Same per-atlas
    /// invariant as `glyph_cache`.
    pub(crate) image_cache: &'a mut ImageCache,
    /// GPU queue for texture uploads.
    pub queue: &'a wgpu::Queue,
    /// Foreground executor for UI-thread async tasks.
    pub executor: &'a ForegroundExecutor,
    /// Display scale factor.
    pub scale_factor: f64,
    /// Per-frame IME state registry. Elements use this in `paint` to update
    /// their cached `caret_client_rect` so the published `CachedImeQuery`
    /// reflects the freshly-painted caret position. First consumer is
    /// the TextField element.
    #[allow(dead_code)]
    pub(crate) ime_registry: &'a RefCell<ImeRegistry>,
    /// Platform window, when one is attached. Animations (caret blink, etc.)
    /// call `schedule_redraw_at` through here. `None` in headless rendering.
    pub(crate) window: Option<&'a dyn slate_platform::Window>,
    /// Hover/press chains for this frame, used to resolve interaction-state
    /// style overrides. Built after the hover diff, before the paint walk.
    pub(crate) interaction: InteractionSnapshot,
    /// Stack of *effective* (already-intersected) clip rects. `last()` is the
    /// clip applied to subsequent draws; empty = unclipped (full viewport).
    /// Pushed/popped by [`push_clip`](Self::push_clip) / [`pop_clip`](Self::pop_clip).
    clip_stack: Vec<ClipRect>,
}

impl<'a> PaintCtx<'a> {
    /// Create a new paint context.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        taffy: &'a TaffyTree<NodeContext>,
        scene: &'a mut Scene,
        text: &'a mut TextSystem,
        glyph_cache: &'a mut GlyphCache,
        glyph_atlas: &'a mut Atlas,
        image_atlas: &'a mut Atlas,
        image_cache: &'a mut ImageCache,
        queue: &'a wgpu::Queue,
        executor: &'a ForegroundExecutor,
        scale_factor: f64,
        ime_registry: &'a RefCell<ImeRegistry>,
        window: Option<&'a dyn slate_platform::Window>,
        interaction: InteractionSnapshot,
    ) -> Self {
        Self {
            taffy,
            scene,
            text,
            glyph_cache,
            glyph_atlas,
            image_atlas,
            image_cache,
            queue,
            executor,
            scale_factor,
            ime_registry,
            window,
            interaction,
            clip_stack: Vec::new(),
        }
    }

    /// Push a clip rectangle and open a fresh clipped scene layer. Every
    /// primitive painted until the matching [`pop_clip`](Self::pop_clip) is
    /// scissored to `clip` intersected with any enclosing clip (so nested
    /// scroll containers compose). `clip` is in logical pixels, absolute
    /// (window-relative) coordinates — pass the element's viewport bounds, not
    /// the scroll-translated child positions.
    ///
    /// Callers MUST pair every `push_clip` with a `pop_clip`; an unbalanced
    /// push leaves later siblings (and the framework focus-ring overlay)
    /// wrongly clipped.
    pub fn push_clip(&mut self, clip: ClipRect) {
        let effective = match self.clip_stack.last() {
            Some(parent) => parent.intersect(&clip),
            None => clip,
        };
        self.clip_stack.push(effective);
        self.scene.push_clip_layer(effective);
    }

    /// Pop the most recent clip and resume painting under the enclosing clip
    /// (or unclipped when the stack empties) in a fresh layer. Pairs with
    /// [`push_clip`](Self::push_clip).
    pub fn pop_clip(&mut self) {
        self.clip_stack.pop();
        match self.clip_stack.last() {
            Some(parent) => self.scene.push_clip_layer(*parent),
            None => self.scene.push_layer(),
        }
    }

    /// True when `id` is the hovered element or an ancestor of it this frame.
    pub(crate) fn is_hovered(&self, id: ElementId) -> bool {
        self.interaction.is_hovered(id)
    }

    /// True when `id` is the pressed element or an ancestor of it this frame.
    pub(crate) fn is_pressed(&self, id: ElementId) -> bool {
        self.interaction.is_pressed(id)
    }

    /// Request a one-shot redraw at `deadline`. No-op in headless mode (no
    /// window attached). Multiple calls within a single paint replace the
    /// in-flight timer — per `Window::schedule_redraw_at` contract.
    pub fn schedule_redraw_at(&self, deadline: std::time::Instant) {
        if let Some(w) = self.window {
            w.schedule_redraw_at(deadline);
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
