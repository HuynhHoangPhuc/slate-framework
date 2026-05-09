use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use parking_lot::Mutex;

use crate::effect::EffectHandle;
use crate::ids::{EffectId, ObserverId, SignalId};

const MAX_EFFECT_ITERATIONS: usize = 100;

/// Trait for memo notification (mark stale).
pub(crate) trait MemoNotify: Send + Sync {
    fn mark_stale(&self);
}

/// Observer kind for dispatch routing.
pub(crate) enum ObserverKind {
    Memo(Arc<dyn MemoNotify>),
    Effect(EffectHandle),
}

/// Central runtime managing reactive state coordination.
///
/// The Runtime handles:
/// - ID allocation for observers and signals
/// - Dirty bit coalescing for redraw requests
/// - Redraw callback installation
/// - Observer registry for dispatch routing
/// - Pending effects queue
///
/// # Thread Safety
///
/// `Runtime` is `Send + Sync`. It can be shared across threads via `Arc<Runtime>`.
/// Background threads can trigger redraws via `mark_dirty()`.
///
/// # Redraw Callback Contract
///
/// The installed redraw callback must not call `Runtime::install_redraw` synchronously.
/// Doing so would deadlock on the internal mutex.
pub struct Runtime {
    next_observer: AtomicU64,
    next_signal: AtomicU64,
    dirty: AtomicBool,
    redraw: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    observers: Mutex<HashMap<ObserverId, ObserverKind>>,
    pending_effects: Mutex<Vec<EffectId>>,
}

impl Runtime {
    /// Creates a new runtime instance.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            next_observer: AtomicU64::new(1),
            next_signal: AtomicU64::new(1),
            dirty: AtomicBool::new(false),
            redraw: Mutex::new(None),
            observers: Mutex::new(HashMap::new()),
            pending_effects: Mutex::new(Vec::new()),
        })
    }

    /// Installs a redraw callback that will be invoked when the dirty bit transitions 0→1.
    ///
    /// The callback is typically wired to wake the UI thread's event loop.
    /// Only one callback can be installed at a time; calling again replaces the previous.
    pub fn install_redraw(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *self.redraw.lock() = Some(f);
    }

    /// Marks the runtime as dirty and invokes the redraw callback if installed.
    ///
    /// The callback is only invoked on the 0→1 dirty transition (coalesced).
    /// Multiple concurrent `mark_dirty()` calls result in at most one callback invocation
    /// until `drain_dirty()` resets the flag.
    ///
    /// # Red Team 2026-05-08 — F9
    ///
    /// The redraw lock is dropped BEFORE invoking the callback to prevent deadlock
    /// if the callback re-enters `install_redraw`.
    pub fn mark_dirty(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            let cb = self.redraw.lock().clone();
            if let Some(cb) = cb {
                cb();
            }
        }
    }

    /// Resets the dirty bit and returns its previous value.
    ///
    /// Typically called at the start of a frame to check if any signals changed
    /// since the last drain.
    pub fn drain_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::AcqRel)
    }

    /// Allocates a new unique observer ID.
    pub fn next_observer_id(&self) -> ObserverId {
        ObserverId::new(self.next_observer.fetch_add(1, Ordering::Relaxed))
    }

    /// Allocates a new unique signal ID.
    pub fn next_signal_id(&self) -> SignalId {
        SignalId::new(self.next_signal.fetch_add(1, Ordering::Relaxed))
    }

    /// Registers a memo observer.
    pub(crate) fn register_observer(&self, id: ObserverId, kind: ObserverKind) {
        self.observers.lock().insert(id, kind);
    }

    /// Registers an effect observer.
    pub(crate) fn register_effect(&self, id: ObserverId, handle: EffectHandle) {
        self.observers
            .lock()
            .insert(id, ObserverKind::Effect(handle));
    }

    /// Unregisters an observer (called on Memo/Effect drop).
    pub(crate) fn unregister_observer(&self, id: ObserverId) {
        self.observers.lock().remove(&id);
    }

    /// Returns the number of registered observers (for testing).
    #[cfg(test)]
    pub fn observers_count(&self) -> usize {
        self.observers.lock().len()
    }

    /// Notifies an observer that one of its dependencies changed.
    ///
    /// # Red Team 2026-05-08 — F10
    ///
    /// The observers lock is dropped BEFORE dispatching to avoid AB-BA deadlock
    /// with cross-signal Memo chains.
    pub fn notify_observer(&self, id: ObserverId) {
        let kind = self.observers.lock().get(&id).map(|k| match k {
            ObserverKind::Memo(m) => ObserverKindClone::Memo(m.clone()),
            ObserverKind::Effect(_) => ObserverKindClone::Effect(id),
        });

        match kind {
            Some(ObserverKindClone::Memo(m)) => m.mark_stale(),
            Some(ObserverKindClone::Effect(_)) => {
                self.pending_effects.lock().push(id);
            }
            None => {
                log::trace!("notify_observer: {:?} not found (already dropped?)", id);
            }
        }
    }

    /// Drains and runs all pending effects.
    ///
    /// This should be called on the UI thread before rendering.
    /// Effects that set signals may enqueue more effects; this method
    /// iterates until the queue is empty or MAX_EFFECT_ITERATIONS is reached.
    ///
    /// # Red Team 2026-05-08 — F12
    ///
    /// Effects store Weak handles. If an effect was dropped between enqueue
    /// and drain, upgrade fails and we skip it silently.
    pub fn drain_effects(&self) {
        for iteration in 0..MAX_EFFECT_ITERATIONS {
            let queued: Vec<EffectId> = std::mem::take(&mut *self.pending_effects.lock());

            if queued.is_empty() {
                return;
            }

            for id in queued {
                let handle = self.observers.lock().get(&id).and_then(|k| match k {
                    ObserverKind::Effect(h) => h.upgrade(),
                    _ => None,
                });

                if let Some(effect) = handle {
                    effect.run();
                }
            }

            if iteration == MAX_EFFECT_ITERATIONS - 1 {
                let remaining = self.pending_effects.lock().len();
                if remaining > 0 {
                    log::warn!(
                        "Effect drain hit max iterations ({}); {} effects still pending. \
                         Check for Effect closures that set their own dependencies.",
                        MAX_EFFECT_ITERATIONS,
                        remaining
                    );
                }
            }
        }
    }
}

#[allow(dead_code)] // Effect variant holds data for dispatch routing
enum ObserverKindClone {
    Memo(Arc<dyn MemoNotify>),
    Effect(ObserverId),
}

impl Default for Runtime {
    fn default() -> Self {
        Self {
            next_observer: AtomicU64::new(1),
            next_signal: AtomicU64::new(1),
            dirty: AtomicBool::new(false),
            redraw: Mutex::new(None),
            observers: Mutex::new(HashMap::new()),
            pending_effects: Mutex::new(Vec::new()),
        }
    }
}

fn _assert_send_sync<T: Send + Sync>() {}

#[allow(dead_code)]
fn _runtime_is_send_sync() {
    _assert_send_sync::<Runtime>();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Runtime>();
    }

    #[test]
    fn next_ids_increment() {
        let rt = Runtime::new();

        let o1 = rt.next_observer_id();
        let o2 = rt.next_observer_id();
        assert_ne!(o1, o2);

        let s1 = rt.next_signal_id();
        let s2 = rt.next_signal_id();
        assert_ne!(s1, s2);
    }

    #[test]
    fn dirty_coalescing() {
        let rt = Runtime::new();

        assert!(!rt.drain_dirty());

        rt.mark_dirty();
        rt.mark_dirty();
        rt.mark_dirty();

        assert!(rt.drain_dirty());
        assert!(!rt.drain_dirty());
    }
}
