//! View trait for element tree generation.
//!
//! Views produce element trees via `render()`. Phase 4 (signals) will make
//! rendering reactive; for now, `render()` is called every frame.

use crate::element::AnyElement;

/// Trait for types that produce element trees.
///
/// Implement this trait to define reusable UI components.
/// `render()` is called once per frame to produce the element tree.
///
/// # Example
///
/// ```ignore
/// struct HelloView {
///     message: String,
/// }
///
/// impl View for HelloView {
///     fn render(&mut self) -> AnyElement {
///         Div::new()
///             .child(Text::new(&self.message))
///             .into_any()
///     }
/// }
/// ```
pub trait View: 'static {
    /// Generate the element tree for this view.
    ///
    /// Called every frame (for now). Phase 4 will make this reactive.
    fn render(&mut self) -> AnyElement;
}

/// Extension trait for converting elements into AnyElement.
///
/// Implemented via `IntoElement`, provides a convenient `.into_any()` method.
pub trait IntoAny {
    fn into_any(self) -> AnyElement;
}

impl<T: crate::element::IntoElement> IntoAny for T {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}
