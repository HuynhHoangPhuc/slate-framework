use std::hash::Hash;

/// Unique identifier for an observer (Memo, Effect, or view scope).
/// Allocated via `Runtime::next_observer_id()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObserverId(pub(crate) u64);

impl ObserverId {
    #[inline]
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Unique identifier for a signal.
/// Allocated via `Runtime::next_signal_id()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SignalId(pub(crate) u64);

impl SignalId {
    #[inline]
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }
}

/// Placeholder for effect identification in pending queue.
/// Effects are identified by their ObserverId.
pub type EffectId = ObserverId;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn ids_are_send_sync() {
        assert_send_sync::<ObserverId>();
        assert_send_sync::<SignalId>();
    }

    #[test]
    fn ids_are_copy() {
        let o = ObserverId::new(42);
        let o2 = o;
        assert_eq!(o, o2);

        let s = SignalId::new(99);
        let s2 = s;
        assert_eq!(s, s2);
    }
}
