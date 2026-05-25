//! Event dispatch implementations on `AppState`.
//!
//! Submodules hold the per-event-class dispatchers. The shared focused-chain
//! helper lives here because keyboard, text-input, and IME dispatchers all
//! walk the same leaf→root ancestor chain.

use smallvec::SmallVec;

use slate_platform::WindowId;

use crate::types::ElementId;

use super::state::AppState;

mod ime;
mod key;
mod mouse;
mod text_input;

impl AppState {
    /// Snapshot the focused element + its ancestor chain (leaf → root) for the
    /// given window.
    ///
    /// Returns an empty `SmallVec` when no element is focused or the window is
    /// unknown, so callers can skip the per-element bubble without paying for
    /// the walk.
    pub(super) fn build_focused_chain(&self, window_id: WindowId) -> SmallVec<[ElementId; 8]> {
        let mut chain: SmallVec<[ElementId; 8]> = SmallVec::new();
        let guard = self.windows.borrow();
        let Some(win) = guard.get(&window_id) else {
            return chain;
        };
        let Some(start) = win.focus_registry.borrow().focused() else {
            return chain;
        };
        let parent_map = win.parent_map.borrow();
        let mut cur = Some(start);
        while let Some(id) = cur {
            chain.push(id);
            cur = parent_map.get(&id).copied();
        }
        chain
    }
}
