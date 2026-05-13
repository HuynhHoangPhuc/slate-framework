//! Renderer observer trait for device-lost recovery notifications.
//!
//! Subscribers implement [`RendererObserver`] to receive callbacks when the
//! GPU device is recreated. Primary use: invalidating CPU-side caches that
//! reference GPU resources (atlas AllocIds, texture handles).
//!
//! # Contract
//!
//! **IMPORTANT:** `on_renderer_recreated` implementations MUST NOT call back
//! into [`Renderer`] methods. Doing so would re-borrow the `observers` RefCell
//! and panic. Implementations should only clear their own internal state.

/// Trait for receiving renderer recreation notifications.
///
/// Implementors are notified after each successful device rebuild. The
/// `generation` counter increments monotonically (1, 2, 3, ...) with each
/// rebuild; consumers can use it to detect stale cached resources.
///
/// # Thread Safety
///
/// This trait is designed for single-threaded use. Observers are stored as
/// `Weak<dyn RendererObserver>` (using `std::rc::Weak`, not `std::sync::Weak`).
pub trait RendererObserver {
    /// Called after the renderer has been successfully recreated.
    ///
    /// `generation` is the new renderer generation (increments on each rebuild).
    /// Implementations should clear any cached GPU resource references (e.g.,
    /// atlas AllocIds, texture handles) that are now invalid.
    ///
    /// # Contract
    ///
    /// - MUST NOT call back into `Renderer` methods (would panic on RefCell borrow)
    /// - SHOULD complete quickly (blocks the recovery path)
    fn on_renderer_recreated(&self, generation: u64);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct CountingObserver {
        count: Cell<u32>,
        last_gen: Cell<u64>,
    }

    impl CountingObserver {
        fn new() -> Self {
            Self {
                count: Cell::new(0),
                last_gen: Cell::new(0),
            }
        }
    }

    impl RendererObserver for CountingObserver {
        fn on_renderer_recreated(&self, generation: u64) {
            self.count.set(self.count.get() + 1);
            self.last_gen.set(generation);
        }
    }

    #[test]
    fn observer_receives_callback() {
        let obs = Rc::new(CountingObserver::new());
        obs.on_renderer_recreated(42);
        assert_eq!(obs.count.get(), 1);
        assert_eq!(obs.last_gen.get(), 42);
    }

    #[test]
    fn weak_upgrade_works() {
        let obs: Rc<dyn RendererObserver> = Rc::new(CountingObserver::new());
        let weak = Rc::downgrade(&obs);
        assert!(weak.upgrade().is_some());
        drop(obs);
        assert!(weak.upgrade().is_none());
    }
}
