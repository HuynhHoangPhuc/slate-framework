//! Core Element trait and type erasure.
//!
//! The Element trait defines a three-phase lifecycle:
//! 1. `request_layout` — create Taffy nodes, measure content
//! 2. `prepaint` — register hit regions and accessibility nodes
//! 3. `paint` — emit GPU primitives to the scene
//!
//! `AnyElement` provides type erasure via `Box<dyn AnyElementDynamic>`.

use crate::context::{LayoutCtx, PaintCtx, PrepaintCtx};
use crate::types::{AccessibilityInfo, Bounds, ElementId, LayoutId};

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

    /// Return accessibility information for this element.
    ///
    /// Default returns `None` — element is invisible to screen readers.
    /// Built-in elements (Div, Text, Button) provide meaningful a11y info.
    ///
    /// # Example
    ///
    /// ```ignore
    /// fn accessibility(&self) -> Option<AccessibilityInfo> {
    ///     Some(AccessibilityInfo {
    ///         role: AccessibilityRole::Button,
    ///         label: Some(self.label.clone()),
    ///         ..Default::default()
    ///     })
    /// }
    /// ```
    fn accessibility(&self) -> Option<AccessibilityInfo> {
        None
    }

    /// Return the stable ElementId allocated during prepaint.
    ///
    /// Returns `None` until prepaint has been called. Override to expose
    /// the ID allocated via `PrepaintCtx::allocate_id`.
    ///
    /// Phase 4 signals use this for subscription identity.
    fn id(&self) -> Option<ElementId> {
        None
    }

    /// Compute a hash of paint-relevant inputs for caching.
    ///
    /// Returns 0 by default (sentinel: caching disabled, always rebuild).
    /// Cacheable elements (`Text`) override to hash content, style, and bounds.
    ///
    /// # Phase 6 (D10) Cache Protocol
    ///
    /// - `hash == 0`: Cache bypass — element always re-paints.
    /// - `hash != 0`: Cache lookup by `(ElementId, hash)`.
    ///
    /// # Strategy L (Leaves Only)
    ///
    /// Only leaf elements (`Text`) override this method. Container elements
    /// (`Div`) return 0 (the default) to avoid stale-child replay issues.
    ///
    /// # F2 Quantization
    ///
    /// Bounds and colors contain `f32` which has no `Hash` impl. Use
    /// `paint_cache::hash_f32` / `hash_bounds` / `hash_color` helpers
    /// with 1/256-px quantization.
    fn paint_input_hash(&self, _bounds: Bounds) -> u64 {
        0
    }
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

    /// Get accessibility info for this element.
    pub fn accessibility(&self) -> Option<AccessibilityInfo> {
        self.element.accessibility()
    }

    /// Get the stable ElementId (available after prepaint).
    pub fn id(&self) -> Option<ElementId> {
        self.element.id()
    }

    /// Get the paint input hash for caching (0 = caching disabled).
    pub fn paint_input_hash(&self, bounds: Bounds) -> u64 {
        self.element.paint_input_hash(bounds)
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
    fn accessibility(&self) -> Option<AccessibilityInfo>;
    fn id(&self) -> Option<ElementId>;
    fn paint_input_hash(&self, bounds: Bounds) -> u64;
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

    fn accessibility(&self) -> Option<AccessibilityInfo> {
        self.element.accessibility()
    }

    fn id(&self) -> Option<ElementId> {
        self.element.id()
    }

    fn paint_input_hash(&self, bounds: Bounds) -> u64 {
        self.element.paint_input_hash(bounds)
    }
}

// Sealed pattern: the `Sealed` trait is `pub(crate)` so external crates cannot
// implement `Element`. Framework-internal impls access it via `crate::element::sealed::Sealed`.
// See: https://rust-lang.github.io/api-guidelines/future-proofing.html#sealed-traits-protect-against-downstream-implementations-c-sealed
pub(crate) use sealed::Sealed;

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
