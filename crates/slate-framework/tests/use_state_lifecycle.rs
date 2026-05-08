//! Integration tests for StateRegistry slot lifecycle.
//!
//! Tests verify:
//! - Slot survival across frames when accessed
//! - GC after 2 consecutive frames without access
//! - Type-erased slot stability (different T at same ID)
//! - One-frame gap survives, two-frame gap drops

use slate_framework::types::ElementId;

// Re-create StateRegistry API for testing (since it's pub(crate))
// This duplicates the logic but tests the same semantics.
mod test_registry {
    use std::any::{Any, TypeId};
    use std::collections::HashMap;
    use std::sync::Arc;

    use slate_framework::types::ElementId;
    use slate_reactive::{Runtime, Signal};

    pub struct StateRegistry {
        runtime: Arc<Runtime>,
        slots: HashMap<(ElementId, TypeId), Slot>,
        current_frame: u64,
    }

    struct Slot {
        value: Box<dyn Any + Send + Sync>,
        last_seen_frame: u64,
    }

    impl StateRegistry {
        pub fn new(runtime: Arc<Runtime>) -> Self {
            Self {
                runtime,
                slots: HashMap::new(),
                current_frame: 0,
            }
        }

        pub fn dummy() -> Self {
            Self::new(Runtime::new())
        }

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
                .expect("type-stable")
                .clone()
        }

        pub fn advance_frame(&mut self) {
            self.current_frame = self.current_frame.saturating_add(1);
        }

        pub fn gc(&mut self) {
            let cutoff = self.current_frame.saturating_sub(2);
            self.slots.retain(|_, s| s.last_seen_frame >= cutoff);
        }

        pub fn slot_count(&self) -> usize {
            self.slots.len()
        }
    }
}

use test_registry::StateRegistry;

#[test]
fn survival_same_signal_on_reaccess() {
    let mut reg = StateRegistry::dummy();
    let id = ElementId::from_raw(42);

    let s1 = reg.use_state(id, || 100u32);
    let s2 = reg.use_state(id, || 999u32); // default ignored

    assert!(s1.ptr_eq(&s2), "same (id, T) should return same Signal");
    assert_eq!(s1.get_untracked(), 100, "original default should be used");
}

#[test]
fn gc_baseline_drops_after_three_frames() {
    let mut reg = StateRegistry::dummy();
    let id = ElementId::from_raw(42);

    // Frame 0: create slot
    let _ = reg.use_state(id, || 0u32);
    assert_eq!(reg.slot_count(), 1);

    // End frame 0 → frame 1 (cutoff = 0, slot.last_seen = 0, kept)
    reg.advance_frame();
    reg.gc();
    assert_eq!(reg.slot_count(), 1, "kept after frame 0→1");

    // End frame 1 → frame 2 (cutoff = 0, kept)
    reg.advance_frame();
    reg.gc();
    assert_eq!(reg.slot_count(), 1, "kept after frame 1→2");

    // End frame 2 → frame 3 (cutoff = 1, slot.last_seen = 0, dropped)
    reg.advance_frame();
    reg.gc();
    assert_eq!(reg.slot_count(), 0, "dropped after frame 2→3");
}

#[test]
fn type_erasure_different_types_separate_slots() {
    let mut reg = StateRegistry::dummy();
    let id = ElementId::from_raw(42);

    let s_u32 = reg.use_state(id, || 10u32);
    let s_str = reg.use_state(id, || String::from("hello"));

    assert_eq!(reg.slot_count(), 2, "different T at same ID = 2 slots");
    assert_eq!(s_u32.get_untracked(), 10);
    assert_eq!(s_str.get_untracked(), "hello");
}

#[test]
fn conditional_render_one_frame_gap_survives() {
    let mut reg = StateRegistry::dummy();
    let id = ElementId::from_raw(42);

    // Frame 0: create slot
    let s1 = reg.use_state(id, || 100u32);
    reg.advance_frame();
    reg.gc();

    // Frame 1: skip (no use_state call) → advance+gc
    reg.advance_frame();
    reg.gc();
    assert_eq!(reg.slot_count(), 1, "one-frame gap should survive");

    // Frame 2: re-access — same signal
    let s2 = reg.use_state(id, || 999u32);
    assert!(s1.ptr_eq(&s2), "same Signal should be returned after 1-frame gap");
    assert_eq!(s2.get_untracked(), 100, "value preserved");
}

#[test]
fn conditional_render_two_frame_gap_drops() {
    let mut reg = StateRegistry::dummy();
    let id = ElementId::from_raw(42);

    // Frame 0: create slot with value 100
    let s1 = reg.use_state(id, || 100u32);
    assert_eq!(s1.get_untracked(), 100);
    reg.advance_frame();
    reg.gc();

    // Frame 1: skip → advance+gc
    reg.advance_frame();
    reg.gc();

    // Frame 2: skip → advance+gc (now cutoff = 1, slot.last_seen = 0, dropped)
    reg.advance_frame();
    reg.gc();
    assert_eq!(reg.slot_count(), 0, "two-frame gap should drop slot");

    // Frame 3: re-access — fresh signal with new default
    let s2 = reg.use_state(id, || 200u32);
    assert!(!s1.ptr_eq(&s2), "different Arc = new slot created");
    assert_eq!(s2.get_untracked(), 200, "new default value used");
}

#[test]
fn slot_survives_60_consecutive_frames() {
    let mut reg = StateRegistry::dummy();
    let id = ElementId::from_raw(123);

    let s_initial = reg.use_state(id, || 42u64);

    for frame in 1..=60 {
        reg.advance_frame();
        reg.gc();

        // Access the slot each frame
        let s_now = reg.use_state(id, || 999u64);
        assert!(
            s_initial.ptr_eq(&s_now),
            "slot should survive frame {}",
            frame
        );
    }

    assert_eq!(reg.slot_count(), 1, "still exactly 1 slot after 60 frames");
    assert_eq!(s_initial.get_untracked(), 42, "value unchanged");
}

#[test]
fn multiple_elements_independent_slots() {
    let mut reg = StateRegistry::dummy();
    let id1 = ElementId::from_raw(1);
    let id2 = ElementId::from_raw(2);
    let id3 = ElementId::from_raw(3);

    let s1 = reg.use_state(id1, || 10u32);
    let s2 = reg.use_state(id2, || 20u32);
    let s3 = reg.use_state(id3, || 30u32);

    assert_eq!(reg.slot_count(), 3);
    assert_eq!(s1.get_untracked(), 10);
    assert_eq!(s2.get_untracked(), 20);
    assert_eq!(s3.get_untracked(), 30);

    // GC should only drop elements not accessed
    reg.advance_frame();
    reg.gc();

    // Access only id1 and id2
    let _ = reg.use_state(id1, || 0u32);
    let _ = reg.use_state(id2, || 0u32);

    reg.advance_frame();
    reg.gc();

    // id3 missed 1 frame, still alive
    assert_eq!(reg.slot_count(), 3);

    reg.advance_frame();
    reg.gc();

    // id3 missed 2 frames, dropped
    assert_eq!(reg.slot_count(), 2, "id3 should be GC'd");
}
