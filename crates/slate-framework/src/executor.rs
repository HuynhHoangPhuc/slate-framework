//! Async executor stubs for Phase 2.
//!
//! Full implementation in Phase 4 (Async Runtime).
//! Phase 2 defines the types so contexts can plumb `&ForegroundExecutor`.

use std::marker::PhantomData;

/// Handle to the foreground (UI-thread) executor.
///
/// Elements receive `&ForegroundExecutor` on all contexts to enable
/// `cx.spawn_local()` for UI-thread async tasks.
///
/// # Thread Safety
///
/// `!Send` — must stay on the UI thread.
///
/// # Phase 4
///
/// Full implementation with smol 2.0.2 comes in Phase 4.
/// This is a stub that satisfies the context type signatures.
pub struct ForegroundExecutor {
    _not_send: PhantomData<*const ()>,
}

impl ForegroundExecutor {
    /// Create a new foreground executor stub.
    ///
    /// Phase 4 replaces this with actual smol executor initialization.
    pub fn new() -> Self {
        Self {
            _not_send: PhantomData,
        }
    }

    /// Spawn a local (non-Send) future on the UI thread.
    ///
    /// # Phase 4
    ///
    /// This is a stub that panics. Phase 4 implements actual task spawning.
    pub fn spawn_local<F>(&self, _future: F)
    where
        F: std::future::Future<Output = ()> + 'static,
    {
        // Phase 4 implements actual spawning
        log::warn!("ForegroundExecutor::spawn_local is a Phase 2 stub; implement in Phase 4");
    }
}

impl Default for ForegroundExecutor {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to trigger UI redraws from background tasks.
///
/// Thread-safe wake handle for the event loop.
///
/// # Phase 4
///
/// Full implementation comes in Phase 4 with signal integration.
#[derive(Clone)]
pub struct RedrawRequester {
    // Phase 4: Arc<dyn Fn() + Send + Sync>
    _placeholder: (),
}

impl RedrawRequester {
    /// Create a new redraw requester stub.
    pub fn new() -> Self {
        Self { _placeholder: () }
    }

    /// Request a UI redraw.
    ///
    /// # Phase 4
    ///
    /// This is a stub. Phase 4 implements actual event loop wake.
    pub fn request(&self) {
        log::warn!("RedrawRequester::request is a Phase 2 stub; implement in Phase 4");
    }
}

impl Default for RedrawRequester {
    fn default() -> Self {
        Self::new()
    }
}
