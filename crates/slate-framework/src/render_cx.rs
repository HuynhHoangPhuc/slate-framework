//! `RenderCx` — the context passed to [`crate::View::render`].
//!
//! Minimal by design: it exposes the originating window's id and is the seam
//! through which reactive reads during render are tracked (the render call runs
//! inside `with_observer`, so `Signal::get` subscribes the view observer as it
//! already does). It deliberately does **not** expose state slots / a hooks API
//! — that surface stays internal (`StateRegistry` is `pub(crate)`).

use slate_platform::WindowId;

/// Context handed to `View::render`.
///
/// Single-window today: `window_id()` returns the one window's id. The id is
/// available for consumers to read; it is never used to key per-window state.
pub struct RenderCx {
    window_id: WindowId,
}

impl RenderCx {
    /// Construct a `RenderCx` for the given window. Crate-private — the render
    /// loop builds this and threads it into `View::render`.
    pub(crate) fn new(window_id: WindowId) -> Self {
        Self { window_id }
    }

    /// The id of the window this view is rendering into.
    pub fn window_id(&self) -> WindowId {
        self.window_id
    }
}
