//! Per-frame registry of ime-capable element ids.
//!
//! Mirrors the [`FocusRegistry`](crate::focus::FocusRegistry) lifecycle:
//! cleared at the start of every prepaint, re-registered per ime-capable
//! element during the prepaint walk, then [`prune_missing`](ImeRegistry::prune_missing)
//! collapses any state belonging to unmounted elements.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::types::ElementId;

use super::state::ImeState;

/// Per-frame map of ime-capable element ids to their `ImeState`. Built up
/// during the prepaint walk; pruned of dead entries after the walk.
#[derive(Default)]
pub struct ImeRegistry {
    states: HashMap<ElementId, Rc<RefCell<ImeState>>>,
    /// Set true whenever a new entry is inserted; consumers can use this as
    /// a coarse change signal (currently informational only).
    dirty: bool,
}

impl ImeRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear the dirty flag without dropping entries. Used by the per-frame
    /// lifecycle: call [`prune_missing`](Self::prune_missing) after the
    /// prepaint walk re-registered the surviving ids; entries for ids that
    /// did NOT re-register are then dropped, preserving the `Rc` for ids
    /// that did re-register so element handlers can still observe the same
    /// state across frames.
    pub fn clear(&mut self) {
        self.dirty = false;
    }

    /// Get-or-insert default `ImeState` for `id`. Returns the shared
    /// `Rc<RefCell<_>>` so the caller can hand it to per-element handlers.
    pub fn register(&mut self, id: ElementId) -> Rc<RefCell<ImeState>> {
        match self.states.entry(id) {
            std::collections::hash_map::Entry::Occupied(e) => e.get().clone(),
            std::collections::hash_map::Entry::Vacant(v) => {
                self.dirty = true;
                v.insert(Rc::new(RefCell::new(ImeState::default()))).clone()
            }
        }
    }

    /// Look up the `ImeState` for `id`, if registered.
    pub fn get(&self, id: ElementId) -> Option<Rc<RefCell<ImeState>>> {
        self.states.get(&id).cloned()
    }

    /// Drop entries whose ids are NOT in `registered_ids`. Called by the
    /// host after the prepaint walk has finished re-registering surviving
    /// ime-capable elements.
    pub fn prune_missing(&mut self, registered_ids: &HashSet<ElementId>) {
        self.states.retain(|id, _| registered_ids.contains(id));
    }

    /// Number of registered ime-capable elements (for tests + debugging).
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// True if no ime-capable elements are registered.
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Clear the `dragging` flag on every registered `ImeState`.
    ///
    /// Called when mouse capture is revoked outside the normal mouse_up path
    /// (alt-tab, modal popup, SetCapture stolen by a sibling, drag-off-window).
    /// Without this, an `ImeState` whose mouse_up never arrived would observe
    /// `dragging == true` forever and re-extend the selection on the next
    /// `mouse_move`, looking like a ghost drag.
    pub fn clear_drag_flags(&self) {
        for state_rc in self.states.values() {
            if let Ok(mut state) = state_rc.try_borrow_mut() {
                state.dragging = false;
                // Drop any in-flight multi-click run: capture was lost between
                // the clicks, so a later click must not be counted as a
                // continuation (it could be seconds and a window away).
                state.click_count = 0;
            }
        }
    }
}

impl std::fmt::Debug for ImeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImeRegistry")
            .field("len", &self.states.len())
            .field("dirty", &self.dirty)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> ElementId {
        ElementId::from_raw(n)
    }

    #[test]
    fn empty_registry_is_empty() {
        let r = ImeRegistry::new();
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
        assert!(r.get(id(1)).is_none());
    }

    #[test]
    fn register_inserts_default_state() {
        let mut r = ImeRegistry::new();
        let s = r.register(id(1));
        assert_eq!(r.len(), 1);
        let borrowed = s.borrow();
        assert!(borrowed.text.is_empty());
        assert_eq!(borrowed.caret, 0);
        assert!(borrowed.preedit.is_none());
    }

    #[test]
    fn register_idempotent_returns_same_rc() {
        let mut r = ImeRegistry::new();
        let s1 = r.register(id(1));
        let s2 = r.register(id(1));
        assert_eq!(r.len(), 1);
        assert!(Rc::ptr_eq(&s1, &s2));
    }

    #[test]
    fn prune_missing_drops_absent_ids() {
        let mut r = ImeRegistry::new();
        r.register(id(1));
        r.register(id(2));
        r.register(id(3));
        let alive: HashSet<ElementId> = [id(1), id(3)].into_iter().collect();
        r.prune_missing(&alive);
        assert_eq!(r.len(), 2);
        assert!(r.get(id(1)).is_some());
        assert!(r.get(id(2)).is_none());
        assert!(r.get(id(3)).is_some());
    }

    #[test]
    fn undo_history_survives_re_registration() {
        use crate::elements::text_edit::undo::{EditOp, EditSnapshot};
        let mut r = ImeRegistry::new();
        let s1 = r.register(id(1));
        {
            let mut state = s1.borrow_mut();
            state.text = "a".to_string();
            state.caret = 1;
            state.undo.record_edit(
                EditOp::Insert,
                EditSnapshot {
                    text: "a".to_string(),
                    caret: 1,
                    anchor: None,
                },
            );
        }
        // Re-register the same id (what every render does). The registry hands
        // back the same Rc, so the recorded edit must still be reachable —
        // the original bug was a fresh stack on every frame.
        let s2 = r.register(id(1));
        assert!(Rc::ptr_eq(&s1, &s2));
        let restored = s2.borrow_mut().undo.undo();
        assert_eq!(restored.map(|s| s.text), Some(String::new()));
    }
}
