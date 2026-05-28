//! View trait for element tree generation.
//!
//! Views produce element trees via `render(cx)`. `render` runs inside the
//! reactive observer scope every frame, so signal reads subscribe the view
//! observer (Strategy-A whole-view rebuild).

use crate::element::AnyElement;
use crate::render_cx::RenderCx;

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
///     fn render(&mut self, _cx: &mut RenderCx) -> AnyElement {
///         Div::new()
///             .child(Text::new(&self.message))
///             .into_any()
///     }
/// }
/// ```
pub trait View: 'static {
    /// Generate the element tree for this view.
    ///
    /// Called every frame inside the view's observer scope. `cx` exposes the
    /// window id and is the reactive seam; it carries no user-facing state slots.
    fn render(&mut self, cx: &mut RenderCx) -> AnyElement;
}

/// Extension trait for converting elements into AnyElement.
///
/// Implemented via `IntoElement`, provides a convenient `.into_any()` method.
pub trait IntoAny {
    /// Consume `self` and wrap it as an [`AnyElement`].
    fn into_any(self) -> AnyElement;
}

impl<T: crate::element::IntoElement> IntoAny for T {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}
