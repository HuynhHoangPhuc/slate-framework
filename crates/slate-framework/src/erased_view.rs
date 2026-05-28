//! Object-safe erasure over the generic [`View`] trait.
//!
//! Each window owns one boxed `ErasedView`. The framework's `AppState` is no
//! longer monomorphised on a single `V: View` — heterogeneous per-window views
//! coexist by being type-erased at the internal `WindowState` boundary.
//!
//! Mirrors the `View` trait exactly. If `View` grows methods, `ErasedView`
//! grows alongside (semver-additive within this crate). Current shape:
//! a single `render` method (see `view.rs:30-36`).
//!
//! [`View`]: crate::view::View

use crate::element::AnyElement;
use crate::render_cx::RenderCx;
use crate::view::View;

/// Object-safe shim mirroring [`View`].
///
/// Implementors are obtained via the blanket impl below: any `V: View + 'static`
/// already satisfies `ErasedView` and can be boxed as `Box<dyn ErasedView>`.
pub trait ErasedView: 'static {
    /// Generate the element tree for the current frame. See [`View::render`].
    fn render(&mut self, cx: &mut RenderCx) -> AnyElement;
}

impl<V: View + 'static> ErasedView for V {
    fn render(&mut self, cx: &mut RenderCx) -> AnyElement {
        View::render(self, cx)
    }
}
