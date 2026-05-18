//! FocusRegistry data-layer unit tests (Phase 9b Phase 1).
//!
//! Cross-platform — no platform deps. Covers tree-order traversal,
//! `tab_index`-driven hoisting, negative-`tab_index` exclusion from Tab cycle
//! but inclusion in programmatic `set_focus`, wrap-around semantics, and
//! `prune_missing` after frame rebuild.

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
fn tab_forward_tree_order_when_all_zero() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.register(entry(2, 0));
    r.register(entry(3, 0));

    assert_eq!(r.shift_forward(), Some(id(1)));
    r.set_focus(id(1));
    assert_eq!(r.shift_forward(), Some(id(2)));
    r.set_focus(id(2));
    assert_eq!(r.shift_forward(), Some(id(3)));
    r.set_focus(id(3));
    // Wraps back to first.
    assert_eq!(r.shift_forward(), Some(id(1)));
}

#[test]
fn tab_backward_tree_order_when_all_zero() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.register(entry(2, 0));
    r.register(entry(3, 0));

    // Backward from None picks last.
    assert_eq!(r.shift_backward(), Some(id(3)));
    r.set_focus(id(3));
    assert_eq!(r.shift_backward(), Some(id(2)));
    r.set_focus(id(2));
    assert_eq!(r.shift_backward(), Some(id(1)));
    r.set_focus(id(1));
    // Wraps to last.
    assert_eq!(r.shift_backward(), Some(id(3)));
}

#[test]
fn positive_tab_index_hoisted_before_zero() {
    // Registration order: A(tab=0), B(tab=2), C(tab=1), D(tab=0)
    // Expected tab order: B(2)? No — W3C: positive tab_index comes FIRST in
    // ascending order, then tab_index=0 in tree order. Plan says
    // "tab_index(1) hoisted before tab_index(0) peers". So:
    //   Expected order: C(1), B(2), A(0), D(0)
    // Sort by tab_index: C(1) < B(2) < A(0) < D(0)? No: 0 < 1 < 2 numerically.
    // The plan says "stable sort by (tab_index, registration_index)" so:
    //   numeric: A(0), D(0), C(1), B(2)
    // That contradicts W3C semantics but the plan is authoritative — use plan.
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0)); // A
    r.register(entry(2, 2)); // B
    r.register(entry(3, 1)); // C
    r.register(entry(4, 0)); // D

    // Forward from None → smallest tab_index, earliest registration → A.
    assert_eq!(r.shift_forward(), Some(id(1)));
    r.set_focus(id(1));
    assert_eq!(r.shift_forward(), Some(id(4))); // D (tab 0)
    r.set_focus(id(4));
    assert_eq!(r.shift_forward(), Some(id(3))); // C (tab 1)
    r.set_focus(id(3));
    assert_eq!(r.shift_forward(), Some(id(2))); // B (tab 2)
}

#[test]
fn negative_tab_index_skipped_by_tab_but_focusable_programmatically() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.register(entry(2, -1));
    r.register(entry(3, 0));

    // Tab cycle skips id(2).
    assert_eq!(r.shift_forward(), Some(id(1)));
    r.set_focus(id(1));
    assert_eq!(r.shift_forward(), Some(id(3)));
    r.set_focus(id(3));
    assert_eq!(r.shift_forward(), Some(id(1))); // wraps past 2

    // But set_focus on negative tab_index still works.
    assert!(r.set_focus(id(2)));
    assert_eq!(r.focused(), Some(id(2)));

    // is_tab_reachable reports false; is_focusable reports true.
    assert!(!r.is_tab_reachable(id(2)));
    assert!(r.is_focusable(id(2)));
}

#[test]
fn prune_missing_clears_focused_when_id_absent() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.set_focus(id(1));
    assert_eq!(r.focused(), Some(id(1)));

    // Simulate next prepaint: clear + re-register without id(1).
    r.clear();
    r.register(entry(2, 0));
    r.prune_missing();

    assert_eq!(r.focused(), None);
}

#[test]
fn prune_missing_keeps_focused_when_id_still_registered() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.set_focus(id(1));

    // Simulate next prepaint where id(1) re-registers.
    r.clear();
    r.register(entry(1, 0));
    r.register(entry(2, 0));
    r.prune_missing();

    assert_eq!(r.focused(), Some(id(1)));
}

#[test]
fn set_focus_returns_false_for_unregistered_id() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    assert!(!r.set_focus(id(999)));
    assert_eq!(r.focused(), None);
}

#[test]
fn forward_wrap_around_at_boundary() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.register(entry(2, 0));
    r.set_focus(id(2));
    assert_eq!(r.shift_forward(), Some(id(1)));
}

#[test]
fn backward_wrap_around_at_boundary() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, 0));
    r.register(entry(2, 0));
    r.set_focus(id(1));
    assert_eq!(r.shift_backward(), Some(id(2)));
}

#[test]
fn no_tab_reachable_returns_none() {
    let mut r = FocusRegistry::new();
    r.register(entry(1, -1));
    r.register(entry(2, -2));
    assert_eq!(r.shift_forward(), None);
    assert_eq!(r.shift_backward(), None);
}

#[test]
fn entry_lookup_returns_metadata() {
    let mut r = FocusRegistry::new();
    r.register(FocusableEntry {
        id: id(1),
        tab_index: 5,
        focus_ring: false,
    });
    let e = r.entry(id(1)).expect("entry present");
    assert_eq!(e.tab_index, 5);
    assert!(!e.focus_ring);
}
