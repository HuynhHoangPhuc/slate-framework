//! Phase-specific contexts for the Element lifecycle.
//!
//! Each phase of the Element lifecycle receives a different context:
//! - `LayoutCtx` — layout computation (Taffy tree, text measurement)
//! - `PrepaintCtx` — hit-test registration, accessibility nodes
//! - `PaintCtx` — GPU scene emission, text rasterization
//!
//! All contexts are `!Send + !Sync` because they hold `&mut TextSystem`
//! which carries `PhantomData<*const ()>`.

use slate_renderer::atlas::Atlas;
use slate_renderer::scene::Scene;
use taffy::TaffyTree;

use crate::executor::ForegroundExecutor;
use crate::text_system::TextSystem;
use crate::types::{AccessibilityNode, HitRegion, NodeContext};

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
/// - Accessibility node registration
/// - Text system for text-related computations
/// - Foreground executor for async tasks
/// - Scale factor
pub struct PrepaintCtx<'a> {
    /// Taffy layout tree (read-only for bounds lookup).
    pub taffy: &'a TaffyTree<NodeContext>,
    /// Hit regions registered during prepaint (Phase 6 dispatches events).
    pub hit_regions: &'a mut Vec<HitRegion>,
    /// Accessibility nodes for the a11y tree (Phase 5).
    pub a11y_nodes: &'a mut Vec<AccessibilityNode>,
    /// Text system (mutable for lazy font loading).
    pub text: &'a mut TextSystem,
    /// Foreground executor for UI-thread async tasks.
    pub executor: &'a ForegroundExecutor,
    /// Display scale factor.
    pub scale_factor: f64,
}

impl<'a> PrepaintCtx<'a> {
    /// Create a new prepaint context.
    pub fn new(
        taffy: &'a TaffyTree<NodeContext>,
        hit_regions: &'a mut Vec<HitRegion>,
        a11y_nodes: &'a mut Vec<AccessibilityNode>,
        text: &'a mut TextSystem,
        executor: &'a ForegroundExecutor,
        scale_factor: f64,
    ) -> Self {
        Self {
            taffy,
            hit_regions,
            a11y_nodes,
            text,
            executor,
            scale_factor,
        }
    }

    /// Register a hit region for pointer event handling.
    pub fn register_hit_region(&mut self, region: HitRegion) {
        self.hit_regions.push(region);
    }

    /// Register an accessibility node.
    pub fn register_a11y_node(&mut self, node: AccessibilityNode) {
        self.a11y_nodes.push(node);
    }
}

/// Context for the `paint` phase.
///
/// Provides access to:
/// - Taffy tree for child bounds lookup (read-only)
/// - GPU scene for primitive emission
/// - Text system for text rasterization
/// - Glyph atlas for texture storage
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
    /// GPU queue for texture uploads.
    pub queue: &'a wgpu::Queue,
    /// Foreground executor for UI-thread async tasks.
    pub executor: &'a ForegroundExecutor,
    /// Display scale factor.
    pub scale_factor: f64,
}

impl<'a> PaintCtx<'a> {
    /// Create a new paint context.
    pub fn new(
        taffy: &'a TaffyTree<NodeContext>,
        scene: &'a mut Scene,
        text: &'a mut TextSystem,
        glyph_atlas: &'a mut Atlas,
        queue: &'a wgpu::Queue,
        executor: &'a ForegroundExecutor,
        scale_factor: f64,
    ) -> Self {
        Self {
            taffy,
            scene,
            text,
            glyph_atlas,
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
}
