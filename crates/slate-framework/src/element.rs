//! Core Element trait and type erasure.
//!
//! The Element trait defines a three-phase lifecycle:
//! 1. `request_layout` — create Taffy nodes, measure content
//! 2. `prepaint` — register hit regions and accessibility nodes
//! 3. `paint` — emit GPU primitives to the scene
//!
//! `AnyElement` provides type erasure via `Box<dyn AnyElementDynamic>`.

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::types::{Bounds, LayoutId};

/// Core trait for UI elements with three-phase lifecycle.
///
/// # Phases
///
/// 1. **request_layout**: Create Taffy layout nodes, return `LayoutId` and
///    per-element layout state. Called top-down during layout computation.
///
/// 2. **prepaint**: After bounds are resolved, register hit regions for
///    pointer events and accessibility nodes. Returns paint state.
///
/// 3. **paint**: Emit GPU primitives (rects, glyphs, images) to the scene.
///    Has access to both layout state and paint state.
///
/// # Associated Types
///
/// - `LayoutState`: Data computed during layout (e.g., shaped text, child bounds)
/// - `PaintState`: Data computed during prepaint (e.g., computed colors, clips)
///
/// Both must be `'static` for type erasure in `AnyElement`.
pub trait Element: 'static {
    /// State produced by `request_layout`, consumed by `prepaint` and `paint`.
    type LayoutState: 'static;

    /// State produced by `prepaint`, consumed by `paint`.
    type PaintState: 'static;

    /// Compute layout for this element.
    ///
    /// Create Taffy nodes via `cx.taffy`, measure text via `cx.text`.
    /// Returns the layout node ID and layout state for later phases.
    fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState);

    /// Register hit regions and prepare for painting.
    ///
    /// Called after layout is complete with resolved bounds.
    /// Register hit regions via `cx.register_hit_region()`.
    fn prepaint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        cx: &mut PrepaintCtx,
    ) -> Self::PaintState;

    /// Emit GPU primitives to the scene.
    ///
    /// Called last with all state available.
    fn paint(
        &mut self,
        bounds: Bounds,
        layout_state: &mut Self::LayoutState,
        paint_state: &mut Self::PaintState,
        cx: &mut PaintCtx,
    );
}

/// Sealed trait marker for `IntoElement`.
///
/// Prevents external implementations to avoid coherence issues (E0119).
mod sealed {
    pub trait Sealed {}
}

/// Conversion trait for types that can become Elements.
///
/// # Why Sealed?
///
/// A blanket `impl<E: Element> IntoElement for E` would conflict with
/// `impl IntoElement for &str` under Rust's coherence rules. Instead,
/// each Element type manually opts in with a one-line impl.
pub trait IntoElement: sealed::Sealed {
    /// The Element type this converts to.
    type Element: Element;

    /// Convert to the Element type.
    fn into_element(self) -> Self::Element;
}

/// Type-erased element wrapper.
///
/// Stores any `Element` implementation with its per-phase state,
/// allowing heterogeneous element trees.
pub struct AnyElement {
    element: Box<dyn AnyElementDynamic>,
}

impl AnyElement {
    /// Create a new AnyElement from any type implementing IntoElement.
    pub fn new<E: IntoElement>(element: E) -> Self {
        let element = element.into_element();
        Self {
            element: Box::new(ElementState {
                element,
                layout_state: None,
                paint_state: None,
            }),
        }
    }

    /// Run the layout phase.
    pub fn request_layout(&mut self, cx: &mut LayoutCtx) -> LayoutId {
        self.element.request_layout(cx)
    }

    /// Run the prepaint phase.
    pub fn prepaint(&mut self, bounds: Bounds, cx: &mut PrepaintCtx) {
        self.element.prepaint(bounds, cx);
    }

    /// Run the paint phase.
    pub fn paint(&mut self, bounds: Bounds, cx: &mut PaintCtx) {
        self.element.paint(bounds, cx);
    }
}

/// Internal trait for type-erased element operations.
///
/// Erases the associated types from `Element` so we can store
/// `Box<dyn AnyElementDynamic>`.
trait AnyElementDynamic: 'static {
    fn request_layout(&mut self, cx: &mut LayoutCtx) -> LayoutId;
    fn prepaint(&mut self, bounds: Bounds, cx: &mut PrepaintCtx);
    fn paint(&mut self, bounds: Bounds, cx: &mut PaintCtx);
}

/// Internal wrapper storing element + per-phase state.
///
/// The `Option` fields are filled progressively:
/// - `layout_state`: filled by `request_layout`
/// - `paint_state`: filled by `prepaint`
struct ElementState<E: Element> {
    element: E,
    layout_state: Option<E::LayoutState>,
    paint_state: Option<E::PaintState>,
}

impl<E: Element> AnyElementDynamic for ElementState<E> {
    fn request_layout(&mut self, cx: &mut LayoutCtx) -> LayoutId {
        let (layout_id, layout_state) = self.element.request_layout(cx);
        self.layout_state = Some(layout_state);
        layout_id
    }

    fn prepaint(&mut self, bounds: Bounds, cx: &mut PrepaintCtx) {
        let layout_state = self
            .layout_state
            .as_mut()
            .expect("prepaint called before request_layout");
        let paint_state = self.element.prepaint(bounds, layout_state, cx);
        self.paint_state = Some(paint_state);
    }

    fn paint(&mut self, bounds: Bounds, cx: &mut PaintCtx) {
        let layout_state = self
            .layout_state
            .as_mut()
            .expect("paint called before request_layout");
        let paint_state = self
            .paint_state
            .as_mut()
            .expect("paint called before prepaint");
        self.element.paint(bounds, layout_state, paint_state, cx);
    }
}

// Re-export the sealed module for element implementations
pub use sealed::Sealed;

#[cfg(test)]
mod tests {
    use super::*;

    // Minimal test element
    struct TestElement;

    impl sealed::Sealed for TestElement {}

    impl Element for TestElement {
        type LayoutState = ();
        type PaintState = ();

        fn request_layout(&mut self, cx: &mut LayoutCtx) -> (LayoutId, Self::LayoutState) {
            let node = cx
                .taffy
                .new_leaf(taffy::Style::default())
                .expect("failed to create node");
            (LayoutId(node), ())
        }

        fn prepaint(
            &mut self,
            _bounds: Bounds,
            _layout_state: &mut Self::LayoutState,
            _cx: &mut PrepaintCtx,
        ) -> Self::PaintState {
        }

        fn paint(
            &mut self,
            _bounds: Bounds,
            _layout_state: &mut Self::LayoutState,
            _paint_state: &mut Self::PaintState,
            _cx: &mut PaintCtx,
        ) {
        }
    }

    impl IntoElement for TestElement {
        type Element = Self;
        fn into_element(self) -> Self {
            self
        }
    }

    #[test]
    fn any_element_creation() {
        let _any = AnyElement::new(TestElement);
    }
}
