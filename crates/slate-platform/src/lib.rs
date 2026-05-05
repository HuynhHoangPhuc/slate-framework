//! Platform abstraction for window + event loop.
//!
//! Implementations: macOS (`objc2-app-kit`), Windows (`windows-rs`).
//! Linux backends are planned post-Phase-0; the trait is shaped so an
//! X11/Wayland impl can plug in without disturbing existing callers.

use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

/// Cross-platform window/event-loop driver.
///
/// One `Platform` per process. `run` enters the OS-native event loop and
/// blocks until [`Platform::quit`] is invoked.
pub trait Platform: 'static {
    type Window: Window;

    /// Initialize the platform layer (e.g. shared `NSApplication` on
    /// macOS, register `WNDCLASS` on Windows). Must be called from the
    /// main OS thread.
    fn new() -> Self
    where
        Self: Sized;

    /// Create a window. The renderer holds a clone of the returned `Arc`
    /// for the surface lifetime.
    fn create_window(&self, opts: WindowOptions) -> Arc<Self::Window>;

    /// Enter the native run loop. Calls `handler` for every dispatched
    /// [`Event`]. Blocks until [`Platform::quit`] returns control.
    ///
    /// Borrows `&self` so the caller can keep the platform handle (and
    /// invoke `quit` from inside the closure). Handler lifetime is tied
    /// to that borrow, *not* `'static`.
    fn run<F>(&self, handler: F)
    where
        F: FnMut(Event);

    /// Request the run loop to exit. Idempotent.
    /// macOS: stops the shared `NSApplication`. Windows: `PostQuitMessage(0)`.
    fn quit(&self);
}

/// Window handle.
///
/// Intentionally **not** `Send + Sync`: AppKit `NSWindow`/`NSView` and
/// Win32 `HWND` are tied to the thread that created them. Cross-thread
/// requests must travel through the (future) event channel rather than
/// a shared `&Window` reference.
pub trait Window: HasWindowHandle + HasDisplayHandle + 'static {
    /// Physical pixel size of the drawable area.
    fn size(&self) -> (u32, u32);
    /// Backing scale factor (1.0 standard, 2.0 Retina, etc.).
    fn scale_factor(&self) -> f64;
    /// Schedule a redraw. The next `Event::WindowRedrawRequested` for
    /// this window will fire after the OS draws the next frame.
    fn request_redraw(&self);
    fn set_title(&self, title: &str);
    /// Begin closing the window. The `WindowDestroyed` event fires
    /// asynchronously when the OS releases native resources.
    fn close(&self);
    /// Stable id for routing events.
    fn id(&self) -> WindowId;
}

#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: String,
    /// Logical size in points.
    pub size: (u32, u32),
    pub min_size: Option<(u32, u32)>,
    pub resizable: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: String::from("slate"),
            size: (800, 600),
            min_size: None,
            resizable: true,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub u64);

#[derive(Debug, Clone)]
pub enum Event {
    /// Sent once after platform init, before the first paint.
    Resumed,
    WindowResized {
        window: WindowId,
        size: (u32, u32),
    },
    WindowCloseRequested {
        window: WindowId,
    },
    WindowDestroyed {
        window: WindowId,
    },
    WindowRedrawRequested {
        window: WindowId,
    },
    /// Sent immediately before `run` returns.
    Exiting,
}

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::{MacPlatform as DefaultPlatform, MacWindow as DefaultWindow};

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
pub use win::{WinPlatform as DefaultPlatform, WinWindow as DefaultWindow};
