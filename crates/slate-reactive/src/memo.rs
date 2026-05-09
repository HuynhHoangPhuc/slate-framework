use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;

use crate::ids::ObserverId;
use crate::observer::{current_observer, observer_stack_depth, with_observer};
use crate::runtime::{MemoNotify, ObserverKind, Runtime};

const MAX_MEMO_DEPTH: usize = 64;

/// A cached derived value that recomputes lazily when dependencies change.
///
/// `Memo<T>` wraps a computation function that depends on signals or other memos.
/// The computed value is cached and only recomputed when a dependency changes.
///
/// # Lazy Recomputation
///
/// When a dependency notifies this memo, it marks itself as stale but does NOT
/// recompute immediately. Recomputation happens on the next `.get()` call.
///
/// # Thread Safety
///
/// `Memo<T>: Send + Sync` when `T: Send + Sync`. The compute function must also
/// be `Send + Sync`.
pub struct Memo<T> {
    inner: Arc<MemoInner<T>>,
}

struct MemoInner<T> {
    runtime: Arc<Runtime>,
    id: ObserverId,
    compute: Box<dyn Fn() -> T + Send + Sync>,
    cache: Mutex<Option<T>>,
    stale: AtomicBool,
    subscribers: Mutex<HashSet<ObserverId>>,
}

impl<T: Send + Sync + Clone + 'static> Memo<T> {
    /// Creates a new memo with the given compute function.
    ///
    /// The function will be called lazily on first `.get()` and whenever
    /// a tracked dependency changes.
    pub fn new(rt: Arc<Runtime>, f: impl Fn() -> T + Send + Sync + 'static) -> Self {
        let id = rt.next_observer_id();
        let inner = Arc::new(MemoInner {
            runtime: rt.clone(),
            id,
            compute: Box::new(f),
            cache: Mutex::new(None),
            stale: AtomicBool::new(true),
            subscribers: Mutex::new(HashSet::new()),
        });

        rt.register_observer(id, ObserverKind::Memo(inner.clone() as Arc<dyn MemoNotify>));

        Self { inner }
    }

    /// Returns the cached value, recomputing if stale.
    ///
    /// If called within an observer scope, that observer will be subscribed
    /// to this memo's changes.
    pub fn get(&self) -> T {
        if let Some(observer) = current_observer() {
            self.inner.subscribers.lock().insert(observer);
        }

        let needs_recompute =
            self.inner.cache.lock().is_none() || self.inner.stale.swap(false, Ordering::AcqRel);

        if needs_recompute {
            if observer_stack_depth() > MAX_MEMO_DEPTH {
                panic!(
                    "Memo recursion depth exceeded {} (memo {:?}). \
                     Check for circular memo dependencies.",
                    MAX_MEMO_DEPTH, self.inner.id
                );
            }

            let value = with_observer(self.inner.id, || (self.inner.compute)());

            let mut cache = match self.inner.cache.try_lock() {
                Some(guard) => guard,
                None => panic!(
                    "Memo cycle detected: memo {:?} attempted to read itself during computation",
                    self.inner.id
                ),
            };
            *cache = Some(value);
        }

        self.inner
            .cache
            .lock()
            .as_ref()
            .expect("cache should be populated")
            .clone()
    }

    /// Returns the cached value without subscribing the current observer.
    pub fn get_untracked(&self) -> T {
        if self.inner.cache.lock().is_none() || self.inner.stale.load(Ordering::Acquire) {
            self.inner.stale.store(false, Ordering::Release);
            let value = with_observer(self.inner.id, || (self.inner.compute)());
            *self.inner.cache.lock() = Some(value);
        }

        self.inner
            .cache
            .lock()
            .as_ref()
            .expect("cache should be populated")
            .clone()
    }

    /// Returns the observer ID for this memo (for testing).
    #[cfg(test)]
    pub(crate) fn observer_id(&self) -> ObserverId {
        self.inner.id
    }
}

impl<T> Clone for Memo<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Drop for MemoInner<T> {
    fn drop(&mut self) {
        self.runtime.unregister_observer(self.id);
    }
}

impl<T: Send + Sync + 'static> MemoNotify for MemoInner<T> {
    fn mark_stale(&self) {
        self.stale.store(true, Ordering::Release);

        let subs: Vec<ObserverId> = { self.subscribers.lock().drain().collect() };

        for s in subs {
            self.runtime.notify_observer(s);
        }
    }
}

fn _assert_send_sync<T: Send + Sync>() {}

#[allow(dead_code)]
fn _memo_is_send_sync() {
    _assert_send_sync::<Memo<u32>>();
    _assert_send_sync::<Memo<String>>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Signal;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn memo_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Memo<u32>>();
        assert_send_sync::<Memo<String>>();
    }

    #[test]
    fn basic_memo() {
        let rt = Runtime::new();
        let memo = Memo::new(rt, || 42 * 2);

        assert_eq!(memo.get_untracked(), 84);
    }

    #[test]
    fn memo_caches_result() {
        let rt = Runtime::new();
        let compute_count = Arc::new(AtomicUsize::new(0));
        let count = compute_count.clone();

        let memo = Memo::new(rt, move || {
            count.fetch_add(1, Ordering::SeqCst);
            100
        });

        assert_eq!(memo.get_untracked(), 100);
        assert_eq!(compute_count.load(Ordering::SeqCst), 1);

        assert_eq!(memo.get_untracked(), 100);
        assert_eq!(compute_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn memo_recomputes_when_stale() {
        let rt = Runtime::new();
        let signal = Signal::new(rt.clone(), 10);
        let s = signal.clone();

        let compute_count = Arc::new(AtomicUsize::new(0));
        let count = compute_count.clone();

        let memo = Memo::new(rt.clone(), move || {
            count.fetch_add(1, Ordering::SeqCst);
            s.get_untracked() * 2
        });

        assert_eq!(memo.get_untracked(), 20);
        assert_eq!(compute_count.load(Ordering::SeqCst), 1);

        rt.notify_observer(memo.observer_id());

        assert_eq!(memo.get_untracked(), 20);
        assert_eq!(compute_count.load(Ordering::SeqCst), 2);
    }
}
