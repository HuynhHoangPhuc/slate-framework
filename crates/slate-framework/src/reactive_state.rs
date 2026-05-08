//! Element-level state registry for reactive state slots.
//!
//! `StateRegistry` maps `(ElementId, TypeId)` to type-erased `Signal<T>` slots.
//! Slots track `last_seen_frame`; slots not accessed for 2+ frames are GC'd.
//!
//! # Visibility
//!
//! This module is `pub(crate)` — internal consumers (e.g., paint cache) use
//! `state_registry.use_state(id, default)` directly. No public hooks-style API
//! ships in v1 per F6 Route A decision (see phase-04 plan).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use slate_reactive::{Runtime, Signal};

use crate::types::ElementId;

/// Per-element state slot registry.
///
/// Maps `(ElementId, TypeId)` → erased `Signal<T>`. Each slot tracks its
/// `last_seen_frame`; slots not accessed for 2 consecutive frames are GC'd
/// by `gc()` (called at frame end).
pub(crate) struct StateRegistry {
    #[allow(dead_code)] // Used in Phase 5 by use_state
    runtime: Arc<Runtime>,
    slots: HashMap<(ElementId, TypeId), Slot>,
    current_frame: u64,
}

/// Type-erased state slot.
struct Slot {
    /// Erased `Signal<T>` — downcast via `TypeId` key.
    #[allow(dead_code)] // Used in Phase 5 by use_state
    value: Box<dyn Any + Send + Sync>,
    /// Frame number when this slot was last accessed via `use_state`.
    last_seen_frame: u64,
}

impl StateRegistry {
    /// Create a new registry backed by the given runtime.
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self {
            runtime,
            slots: HashMap::new(),
            current_frame: 0,
        }
    }

    /// Create a dummy registry for headless/test contexts.
    ///
    /// Uses a fresh runtime with no external wiring.
    pub fn dummy() -> Self {
        Self::new(Runtime::new())
    }

    /// Get or create a state slot for the given element ID.
    ///
    /// - Same `(id, T)` across frames → returns the same `Signal<T>` (state persists).
    /// - Different `T` at same `id` → different slot (key includes `TypeId`).
    /// - Updates `last_seen_frame` on access.
    ///
    /// # Panics
    ///
    /// Panics if the stored value for `(id, TypeId::of::<T>())` is not a `Signal<T>`.
    /// This indicates a bug — the key includes `TypeId`, so type mismatch is impossible
    /// unless the caller corrupted the registry.
    #[allow(dead_code)] // Used in Phase 6 by paint cache
    pub fn use_state<T: Send + Sync + 'static>(
        &mut self,
        id: ElementId,
        default: impl FnOnce() -> T,
    ) -> Signal<T> {
        let key = (id, TypeId::of::<T>());
        let cf = self.current_frame;
        let rt = self.runtime.clone();
        let slot = self.slots.entry(key).or_insert_with(move || Slot {
            value: Box::new(Signal::new(rt, default())),
            last_seen_frame: cf,
        });
        slot.last_seen_frame = self.current_frame;
        slot.value
            .downcast_ref::<Signal<T>>()
            .expect("StateRegistry type mismatch — key includes TypeId, this is a bug")
            .clone()
    }

    /// Advance the frame counter.
    ///
    /// Call at the end of each frame, before `gc()`.
    pub fn advance_frame(&mut self) {
        self.current_frame = self.current_frame.saturating_add(1);
    }

    /// Garbage-collect slots not seen for 2+ frames.
    ///
    /// A slot with `last_seen_frame = N` survives:
    /// - Frame N+1 (cutoff = N-1 ≤ N)
    /// - Dropped after frame N+2 (cutoff = N+1 > N)
    ///
    /// Call at the end of each frame, after `advance_frame()`.
    pub fn gc(&mut self) {
        let cutoff = self.current_frame.saturating_sub(2);
        self.slots.retain(|_, s| s.last_seen_frame >= cutoff);
    }

    /// Current frame number (for testing/debugging).
    #[cfg(test)]
    #[allow(dead_code)]
    pub fn current_frame(&self) -> u64 {
        self.current_frame
    }

    /// Number of active slots (for testing).
    #[cfg(test)]
    pub fn slot_count(&self) -> usize {
        self.slots.len()
    }

    /// Access the backing runtime (for internal use).
    #[allow(dead_code)] // Used in Phase 5 for runtime access
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.runtime
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_state_returns_same_signal_on_reaccess() {
        let mut reg = StateRegistry::dummy();
        let id = ElementId::from_hash(42);

        let s1 = reg.use_state(id, || 100u32);
        let s2 = reg.use_state(id, || 999u32); // default ignored

        assert!(s1.ptr_eq(&s2));
        assert_eq!(s1.get_untracked(), 100);
    }

    #[test]
    fn different_types_get_different_slots() {
        let mut reg = StateRegistry::dummy();
        let id = ElementId::from_hash(42);

        let s_u32 = reg.use_state(id, || 10u32);
        let s_str = reg.use_state(id, || String::from("hello"));

        assert_eq!(reg.slot_count(), 2);
        assert_eq!(s_u32.get_untracked(), 10);
        assert_eq!(s_str.get_untracked(), "hello");
    }

    #[test]
    fn gc_drops_stale_slots() {
        let mut reg = StateRegistry::dummy();
        let id = ElementId::from_hash(42);

        // Frame 0: create slot
        let _ = reg.use_state(id, || 1u32);
        assert_eq!(reg.slot_count(), 1);

        // End frame 0 → frame 1 (cutoff = 0, slot.last_seen = 0, kept)
        reg.advance_frame();
        reg.gc();
        assert_eq!(reg.slot_count(), 1);

        // End frame 1 → frame 2 (cutoff = 0, kept)
        reg.advance_frame();
        reg.gc();
        assert_eq!(reg.slot_count(), 1);

        // End frame 2 → frame 3 (cutoff = 1, slot.last_seen = 0, dropped)
        reg.advance_frame();
        reg.gc();
        assert_eq!(reg.slot_count(), 0);
    }

    #[test]
    fn one_frame_gap_survives() {
        let mut reg = StateRegistry::dummy();
        let id = ElementId::from_hash(42);

        // Frame 0: create slot
        let s1 = reg.use_state(id, || 100u32);
        reg.advance_frame();
        reg.gc();

        // Frame 1: skip (no use_state call)
        reg.advance_frame();
        reg.gc();
        assert_eq!(reg.slot_count(), 1, "one-frame gap should survive");

        // Frame 2: re-access — same signal
        let s2 = reg.use_state(id, || 999u32);
        assert!(s1.ptr_eq(&s2));
    }

    #[test]
    fn two_frame_gap_drops() {
        let mut reg = StateRegistry::dummy();
        let id = ElementId::from_hash(42);

        // Frame 0: create slot
        let s1 = reg.use_state(id, || 100u32);
        reg.advance_frame();
        reg.gc();

        // Frame 1: skip
        reg.advance_frame();
        reg.gc();

        // Frame 2: skip
        reg.advance_frame();
        reg.gc();
        assert_eq!(reg.slot_count(), 0, "two-frame gap should drop slot");

        // Frame 3: re-access — fresh signal with default
        let s2 = reg.use_state(id, || 200u32);
        assert!(!s1.ptr_eq(&s2));
        assert_eq!(s2.get_untracked(), 200);
    }
}
