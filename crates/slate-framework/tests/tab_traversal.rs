//! Tab traversal + focusability semantics tests (Phase 9b).
//!
//! Cross-platform — exercises `FocusRegistry` directly. Tab traversal is a
//! pure-data operation; the dispatch wiring (Tab default shift, mouse-down
//! auto-focus) is verified via the existing `focus_registry.rs` unit tests
//! plus the integration tests in `focus_dispatch.rs`.

use slate_framework::types::ElementId;
use slate_framework::{FocusRegistry, FocusableEntry};

fn id(n: u64) -> ElementId {
    ElementId::from_raw(n)
}

fn entry(n: u64, tab_index: i32) -> FocusableEntry {
    FocusableEntry {
        id: id(n),
        tab_index,
        focus_ring: true,
    }
}

#[test]
fn tab_cycles_in_tree_order_when_all_zero() {
    let mut r = FocusRegistry::new();
    r.register(entry(10, 0));
    r.register(entry(11, 0));
    r.register(entry(12, 0));

    assert_eq!(r.shift_forward(), Some(id(10)));
    r.set_focus(id(10));
    assert_eq!(r.shift_forward(), Some(id(11)));
    r.set_focus(id(11));
    assert_eq!(r.shift_forward(), Some(id(12)));
    r.set_focus(id(12));
    assert_eq!(r.shift_forward(), Some(id(10)));
}

#[test]
fn tab_index_sorts_ascending_with_registration_tiebreak() {
    // Sort key is tab_index ascending, with registration order as stable tiebreak.
    // Zero comes before positive (numerically smaller).
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.register(entry(2, 2));
    r.register(entry(3, 0));
    r.register(entry(4, 1));

    assert_eq!(r.shift_forward(), Some(id(1)));
    r.set_focus(id(1));
    assert_eq!(r.shift_forward(), Some(id(3)));
    r.set_focus(id(3));
    assert_eq!(r.shift_forward(), Some(id(4)));
    r.set_focus(id(4));
    assert_eq!(r.shift_forward(), Some(id(2)));
}

#[test]
fn negative_tab_index_excluded_from_tab_cycle() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.register(entry(2, -1));
    r.register(entry(3, 0));

    let mut seen = Vec::new();
    for _ in 0..4 {
        let next = r.shift_forward().unwrap();
        seen.push(next);
        r.set_focus(next);
    }
    assert!(
        !seen.contains(&id(2)),
        "tab_index(-1) must be skipped by Tab traversal"
    );
}

#[test]
fn set_focus_works_on_negative_tab_index_element() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.register(entry(2, -1));

    assert!(r.set_focus(id(2)));
    assert_eq!(r.focused(), Some(id(2)));
}

#[test]
fn shift_backward_cycles_reverse() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.register(entry(2, 0));
    r.register(entry(3, 0));

    assert_eq!(r.shift_backward(), Some(id(3)));
    r.set_focus(id(3));
    assert_eq!(r.shift_backward(), Some(id(2)));
    r.set_focus(id(2));
    assert_eq!(r.shift_backward(), Some(id(1)));
}

#[test]
fn prune_missing_drops_focus_when_focused_element_removed() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.register(entry(2, 0));
    r.set_focus(id(2));
    assert_eq!(r.focused(), Some(id(2)));

    r.clear();
    r.register(entry(1, 0));
    r.prune_missing();

    assert_eq!(
        r.focused(),
        None,
        "focus must clear when focused element disappears"
    );
}

#[test]
fn focus_ring_flag_preserved_on_entry() {
    let mut r = FocusRegistry::new();
    r.register(FocusableEntry {
        id: id(1),
        tab_index: 0,
        focus_ring: false,
    });
    r.register(FocusableEntry {
        id: id(2),
        tab_index: 0,
        focus_ring: true,
    });

    assert_eq!(r.entry(id(1)).map(|e| e.focus_ring), Some(false));
    assert_eq!(r.entry(id(2)).map(|e| e.focus_ring), Some(true));
}
