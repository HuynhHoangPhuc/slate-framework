use std::marker::PhantomData;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::ids::ObserverId;
use crate::observer::with_observer;
use crate::owner::ReactiveOwner;
use crate::runtime::Runtime;

/// A UI-thread side effect that re-runs when dependencies change.
///
/// `Effect` is `!Send` because it contains a `PhantomData<*const ()>` marker.
/// Effects must be created on the UI thread and their closures run on
/// the UI thread during `Runtime::drain_effects()`.
///
/// # Initial Run
///
/// The effect closure runs once immediately upon construction inside
/// its own observer scope, establishing initial subscriptions.
///
/// # Ownership
///
/// Effects register with the current `ReactiveOwner`. When the owner
/// is dropped, the effect is disposed and will not run again.
pub struct Effect {
    _handle: Arc<EffectInner>,
    _not_send: PhantomData<*const ()>,
}

pub(crate) struct EffectInner {
    runtime: Arc<Runtime>,
    id: ObserverId,
    closure: Mutex<Option<Box<dyn FnMut() + Send>>>,
}

unsafe impl Send for EffectInner {}
unsafe impl Sync for EffectInner {}

/// Weak handle stored in the runtime's observer registry.
/// Allows the effect to be garbage collected when dropped.
pub(crate) struct EffectHandle {
    inner: std::sync::Weak<EffectInner>,
}

unsafe impl Send for EffectHandle {}
unsafe impl Sync for EffectHandle {}

impl EffectHandle {
    pub(crate) fn new(inner: &Arc<EffectInner>) -> Self {
        Self {
            inner: Arc::downgrade(inner),
        }
    }

    pub(crate) fn upgrade(&self) -> Option<Arc<EffectInner>> {
        self.inner.upgrade()
    }
}

impl Effect {
    /// Creates a new effect with the given closure.
    ///
    /// The closure runs immediately and will re-run whenever a tracked
    /// dependency changes (after `Runtime::drain_effects()` is called).
    ///
    /// The closure must be `Send` to allow storage in the thread-safe registry,
    /// but it will only be executed on the UI thread.
    pub fn new(rt: Arc<Runtime>, f: impl FnMut() + Send + 'static) -> Self {
        let id = rt.next_observer_id();

        let inner = Arc::new(EffectInner {
            runtime: rt.clone(),
            id,
            closure: Mutex::new(Some(Box::new(f))),
        });

        rt.register_effect(id, EffectHandle::new(&inner));

        with_observer(id, || {
            if let Some(closure) = inner.closure.lock().as_mut() {
                closure();
            }
        });

        if let Some(owner) = ReactiveOwner::current() {
            owner.own_arc(inner.clone());
        }

        Self {
            _handle: inner,
            _not_send: PhantomData,
        }
    }

    /// Returns the observer ID for this effect (for testing).
    #[cfg(test)]
    pub(crate) fn observer_id(&self) -> ObserverId {
        self._handle.id
    }
}

impl EffectInner {
    /// Runs the effect closure inside its observer scope.
    pub(crate) fn run(&self) {
        with_observer(self.id, || {
            if let Some(closure) = self.closure.lock().as_mut() {
                closure();
            }
        });
    }
}

impl Drop for EffectInner {
    fn drop(&mut self) {
        self.runtime.unregister_observer(self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::owner::ReactiveOwner;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn effect_runs_on_creation() {
        let rt = Runtime::new();
        let ran = Arc::new(AtomicUsize::new(0));
        let r = ran.clone();

        let owner = ReactiveOwner::root();
        let _guard = owner.enter();

        let _effect = Effect::new(rt, move || {
            r.fetch_add(1, Ordering::SeqCst);
        });

        assert_eq!(ran.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn effect_not_send() {
        fn is_not_send<T>() {}
        is_not_send::<Effect>();
    }

    #[test]
    fn effect_with_owner() {
        let rt = Runtime::new();
        let run_count = Arc::new(AtomicUsize::new(0));
        let r = run_count.clone();

        let owner = ReactiveOwner::root();
        {
            let _guard = owner.enter();
            let _effect = Effect::new(rt.clone(), move || {
                r.fetch_add(1, Ordering::SeqCst);
            });

            assert_eq!(run_count.load(Ordering::SeqCst), 1);
        }
    }
}
