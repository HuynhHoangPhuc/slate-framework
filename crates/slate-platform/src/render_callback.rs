//! Resize sync callback infrastructure for artifact-free live resize.
//!
//! Platform code invokes `dispatch_resize_sync` from OS resize callbacks
//! (macOS `setFrameSize:`, Windows `WM_NCCALCSIZE`/`WM_SIZE`) to run the
//! layout + GPU pipeline synchronously before the compositor commits.

use crate::WindowId;

/// Physical pixel dimensions of a drawable surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct PhysicalSize {
    pub width: u32,
    pub height: u32,
}

impl PhysicalSize {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// Convert to tuple form for interop with existing APIs.
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

/// Callback type for synchronous resize rendering.
///
/// Registered by the framework via `set_render_callback`, invoked by platform
/// code from inside OS resize callbacks. The callback runs the same layout +
/// GPU pipeline that `Event::WindowRedrawRequested` uses, but inline before
/// the OS commits the new window bounds.
///
/// # Thread Safety
/// Not `Send + Sync` — platform windows are main-thread-only. The callback is
/// stored in a `thread_local!` and invoked from the main thread.
pub type ResizeSyncCallback = Box<dyn FnMut(WindowId, PhysicalSize) + 'static>;
