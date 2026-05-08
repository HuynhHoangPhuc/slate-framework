use std::collections::HashSet;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::ids::ObserverId;
use crate::observer::current_observer;
use crate::runtime::Runtime;

/// A reactive value container that notifies observers when changed.
///
/// `Signal<T>` is the core reactive primitive. Background threads can call
/// `set()` to update the value; any observer that previously called `get()`
/// will be notified and the runtime's dirty bit will be set.
///
/// # Thread Safety
///
/// `Signal<T>: Send + Sync` when `T: Send + Sync`. The value is protected by
/// `RwLock` and the subscriber set by `Mutex`.
///
/// # Subscription Semantics (Strategy A)
///
/// Subscribers are drained on `set()` and must re-subscribe via `get()` on the
/// next render. This is intentional for Strategy A whole-view rebuild where
/// `View::render` runs inside `with_observer(view_observer_id, ...)` every frame.
///
/// **Warning**: This drain-on-set pattern breaks under Strategy B partial rebuild.
/// See `slate-reactive/README.md` for details.
pub struct Signal<T> {
    inner: Arc<SignalInner<T>>,
}

struct SignalInner<T> {
    value: RwLock<T>,
    subscribers: Mutex<HashSet<ObserverId>>,
    runtime: Arc<Runtime>,
}

impl<T: Send + Sync + 'static> Signal<T> {
    /// Creates a new signal with the given initial value.
    pub fn new(rt: Arc<Runtime>, value: T) -> Self {
        Self {
            inner: Arc::new(SignalInner {
                value: RwLock::new(value),
                subscribers: Mutex::new(HashSet::new()),
                runtime: rt,
            }),
        }
    }

    /// Returns a clone of the current value, subscribing the current observer.
    ///
    /// If called within a `with_observer` scope, the observer will be notified
    /// when this signal changes. If called outside any observer scope, the value
    /// is returned without subscription (tracking is opt-in).
    pub fn get(&self) -> T
    where
        T: Clone,
    {
        if let Some(observer) = current_observer() {
            self.inner.subscribers.lock().insert(observer);
        }
        self.inner.value.read().clone()
    }

    /// Returns Some(value) if called within an observer scope, None otherwise.
    ///
    /// Useful for conditional tracking where you want to explicitly handle
    /// the non-reactive case.
    pub fn try_get(&self) -> Option<T>
    where
        T: Clone,
    {
        if current_observer().is_some() {
            Some(self.get())
        } else {
            None
        }
    }

    /// Returns a clone of the current value without subscribing any observer.
    ///
    /// Use this for reads that should not trigger re-renders, such as
    /// background thread reads or debugging.
    pub fn get_untracked(&self) -> T
    where
        T: Clone,
    {
        self.inner.value.read().clone()
    }

    /// Sets the signal to a new value and notifies all subscribers.
    ///
    /// Subscribers are drained (removed) after notification. Under Strategy A
    /// whole-view rebuild, observers re-subscribe on the next `get()` during render.
    ///
    /// This method can be called from any thread.
    pub fn set(&self, v: T) {
        *self.inner.value.write() = v;
        self.notify_subscribers();
    }

    /// Updates the value in-place using a closure and notifies subscribers.
    ///
    /// The closure receives `&mut T` and can modify it. After the closure
    /// completes, all subscribers are notified.
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        f(&mut *self.inner.value.write());
        self.notify_subscribers();
    }

    /// Drains subscribers and notifies them, then marks the runtime dirty.
    fn notify_subscribers(&self) {
        let subs: Vec<ObserverId> = {
            let mut g = self.inner.subscribers.lock();
            g.drain().collect()
        };

        for s in subs {
            self.inner.runtime.notify_observer(s);
        }

        self.inner.runtime.mark_dirty();
    }

    /// Returns the number of current subscribers (for testing).
    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        self.inner.subscribers.lock().len()
    }

    /// Returns true if two signals point to the same underlying storage.
    ///
    /// Useful for verifying that `StateRegistry::use_state` returns the same
    /// signal across frames.
    pub fn ptr_eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

fn _assert_send_sync<T: Send + Sync>() {}

#[allow(dead_code)]
fn _signal_is_send_sync() {
    _assert_send_sync::<Signal<u32>>();
    _assert_send_sync::<Signal<String>>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::with_observer;

    #[test]
    fn signal_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Signal<u32>>();
        assert_send_sync::<Signal<String>>();
        assert_send_sync::<Signal<Vec<u8>>>();
    }

    #[test]
    fn basic_get_set() {
        let rt = Runtime::new();
        let signal = Signal::new(rt, 42);

        assert_eq!(signal.get_untracked(), 42);

        signal.set(100);
        assert_eq!(signal.get_untracked(), 100);
    }

    #[test]
    fn update_in_place() {
        let rt = Runtime::new();
        let signal = Signal::new(rt, vec![1, 2, 3]);

        signal.update(|v| v.push(4));
        assert_eq!(signal.get_untracked(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn subscription_tracking() {
        let rt = Runtime::new();
        let observer = rt.next_observer_id();
        let signal = Signal::new(rt.clone(), 0);

        assert_eq!(signal.subscriber_count(), 0);

        with_observer(observer, || {
            let _ = signal.get();
        });

        assert_eq!(signal.subscriber_count(), 1);

        signal.set(1);
        assert_eq!(signal.subscriber_count(), 0);
    }

    #[test]
    fn clone_shares_inner() {
        let rt = Runtime::new();
        let s1 = Signal::new(rt, 10);
        let s2 = s1.clone();

        s1.set(20);
        assert_eq!(s2.get_untracked(), 20);

        s2.set(30);
        assert_eq!(s1.get_untracked(), 30);
    }

    #[test]
    fn try_get_returns_none_outside_scope() {
        let rt = Runtime::new();
        let signal = Signal::new(rt, 42);

        assert_eq!(signal.try_get(), None);
    }

    #[test]
    fn try_get_returns_some_inside_scope() {
        let rt = Runtime::new();
        let observer = rt.next_observer_id();
        let signal = Signal::new(rt.clone(), 42);

        with_observer(observer, || {
            assert_eq!(signal.try_get(), Some(42));
        });
    }
}
