//! Sync render boundary trait — invoked from platform OS callbacks
//! (Windows `WM_SIZE`/`WM_PAINT`, macOS `setFrameSize:`/`drawRect:`)
//! BEFORE the framebuffer commits.
//!
//! # Ownership
//!
//! `WindowRenderDelegate` is held by platform [`Window`](crate::Window) implementations
//! as `Weak<dyn WindowRenderDelegate>` (installed via
//! [`Window::set_render_delegate`](crate::Window)). The framework's `AppState<V>`
//! implements this trait to run the full layout + GPU pipeline
//! synchronously inside the OS resize/redraw callback. Holding `Weak`
//! prevents the `AppState ↔ Window` reference cycle that would otherwise
//! arise from `AppState` owning `Arc<Window>` strong.
//!
//! # Threading (NOT `Send + Sync` — by design)
//!
//! - **Trait has no `Send`/`Sync` bound.** Implementers may use interior
//!   mutability (`RefCell`, `Cell`) freely.
//! - **All trait methods are invoked on the main thread**, from inside
//!   the OS platform callback. The framework guarantees this; cross-thread
//!   redraws use the async `RedrawRequester` mechanism, not the delegate.
//! - On macOS, `DisplayLink` fires on a high-priority background thread;
//!   the platform code is responsible for marshalling the callback to the
//!   main queue (via `Queue::main().exec_async`) BEFORE invoking the
//!   delegate.
//! - **Implementers must not `Send` themselves across threads** to bypass
//!   this — the trait does not advertise `Send`, and `Rc<AppState>`
//!   cannot cross thread boundaries anyway.
//!
//! # Re-entrancy
//!
//! Methods take `&self` (not `&mut`) so the implementer can use interior
//! mutability. Do NOT re-enter the same `Window`'s delegate synchronously
//! from inside a delegate method — the framework's AppState guards
//! against this via a `rendering: Cell<bool>` flag, but other implementers
//! must follow the same discipline.

use crate::WindowId;

/// Physical pixel dimensions of a window's drawable surface.
///
/// Distinct from logical size (which is divided by scale factor).
/// Phase 4 dispatch sites pass the post-DPI physical extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct PhysicalSize {
    pub width: u32,
    pub height: u32,
}

impl PhysicalSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Convert to `(width, height)` tuple as expected by renderer APIs.
    pub const fn as_tuple(self) -> (u32, u32) {
        (self.width, self.height)
    }
}

impl From<(u32, u32)> for PhysicalSize {
    fn from((width, height): (u32, u32)) -> Self {
        Self { width, height }
    }
}

impl From<PhysicalSize> for (u32, u32) {
    fn from(size: PhysicalSize) -> Self {
        (size.width, size.height)
    }
}

/// Receives synchronous render notifications from a [`Window`](crate::Window).
///
/// Invoked from OS resize callbacks (Windows `WM_SIZE`/`WM_NCCALCSIZE`/
/// `WM_DPICHANGED`, macOS `windowDidResize:` /
/// `windowDidChangeBackingProperties:`) and redraw callbacks
/// (`WM_PAINT`, `drawRect:`, DisplayLink-driven redraw) **before the
/// framebuffer commits**.
///
/// The framework's `AppState<V>` is the canonical implementer. Platform
/// `Window` impls hold a `Weak<dyn WindowRenderDelegate>` to avoid
/// `AppState` ↔ `Window` reference cycles.
///
/// # Re-entrancy
///
/// Methods take `&self` (not `&mut`) so the implementer can use interior
/// mutability (`RefCell`). Do not re-enter the same `Window` synchronously
/// from inside a delegate method.
///
/// # Threading
///
/// See the module-level docs. Methods are main-thread-only.
pub trait WindowRenderDelegate {
    /// Called by the platform when the window's drawable surface must resize.
    /// Implementer should run layout + GPU submit inside this call so the
    /// new framebuffer is ready before the OS commits the new bounds.
    fn on_resize_sync(&self, window_id: WindowId, new_size: PhysicalSize);

    /// Called by the platform when the window needs to redraw (paint cycle).
    /// Implementer runs the full render pipeline synchronously.
    fn on_redraw(&self, window_id: WindowId);
}
