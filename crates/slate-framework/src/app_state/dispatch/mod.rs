//! Event dispatch implementations on `AppState`.
//!
//! Submodules hold the per-event-class dispatchers. The shared focused-chain
//! helper lives here because keyboard, text-input, and IME dispatchers all
//! walk the same leaf→root ancestor chain.

use smallvec::SmallVec;

use crate::types::ElementId;
use crate::view::View;

use super::state::AppState;

mod key;

impl<V: View> AppState<V> {
    /// Snapshot the focused element + its ancestor chain (leaf → root).
    ///
    /// Returns an empty SmallVec when no element is focused, so the caller
    /// can skip the per-element bubble entirely without paying for the walk.
    pub(super) fn build_focused_chain(&self) -> SmallVec<[ElementId; 8]> {
        let mut chain: SmallVec<[ElementId; 8]> = SmallVec::new();
        let Some(start) = self.focus_registry.borrow().focused() else {
            return chain;
        };
        let parent_map = self.parent_map.borrow();
        let mut cur = Some(start);
        while let Some(id) = cur {
            chain.push(id);
            cur = parent_map.get(&id).copied();
        }
        chain
    }
}
