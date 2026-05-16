//! Focus management — Phase 9b foundation.
//!
//! Pure-data layer: types + tab-order traversal. No dispatch wiring lives here;
//! `AppState` (Phase 3) owns a `RefCell<FocusRegistry>` and calls into this
//! module from its event-dispatch pipeline.
//!
//! # Tab order
//! Stable sort by `(tab_index, registration_index)`. Entries with
//! `tab_index < 0` are excluded from the Tab cycle but remain focusable
//! programmatically via [`FocusRegistry::set_focus`].
//!
//! # Per-frame lifecycle
//! [`FocusRegistry::clear`] at start of every prepaint, then each focusable
//! element re-registers via [`FocusRegistry::register`]. After the prepaint
//! walk completes, AppState calls [`FocusRegistry::prune_missing`] to clear
//! `focused` if the previously-focused element was unmounted.
//!
//! Implementation note: `sorted_indices` is rebuilt lazily only when
//! `dirty` is set by a `register` call; Tab shifts iterate it without
//! allocating.
//!
//! See `plans/260516-2000-slate-phase-9b-per-element-keys-and-focus/phase-01-*.md`.

use crate::types::ElementId;

/// Per-element focus metadata captured during prepaint.
#[derive(Copy, Clone, Debug)]
pub struct FocusableEntry {
    /// Stable id of the focusable element.
    pub id: ElementId,
    /// W3C-style `tab_index`. `< 0` => not in Tab cycle, but `set_focus` still works.
    pub tab_index: i32,
    /// Whether the framework's hardcoded focus ring should render for this entry.
    pub focus_ring: bool,
}

/// Owns the per-frame focusable list plus the current focused id.
///
/// Built up each prepaint via [`register`](Self::register) calls; consumed by
/// the keyboard dispatch path and the focus-ring paint pass.
pub struct FocusRegistry {
    entries: Vec<FocusableEntry>,
    focused: Option<ElementId>,
    /// True when `entries` has changed since the last sort.
    dirty: bool,
    /// Indices into `entries`, stable-sorted by `(tab_index, registration_index)`.
    /// Includes negative-tab_index entries; Tab traversal filters them out.
    sorted_indices: Vec<usize>,
}

impl FocusRegistry {
    /// Create an empty registry with no focused element.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            focused: None,
            dirty: false,
            sorted_indices: Vec::new(),
        }
    }

    /// Clear all entries (call at start of every prepaint).
    ///
    /// Does NOT clear `focused` — call [`prune_missing`](Self::prune_missing)
    /// after re-registration to auto-clear if focused was unmounted.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.sorted_indices.clear();
        self.dirty = false;
    }

    /// Register a focusable element during prepaint.
    pub fn register(&mut self, entry: FocusableEntry) {
        self.entries.push(entry);
        self.dirty = true;
    }

    /// Set `focused` to `id` if `id` is registered.
    ///
    /// Returns `true` when `id` is in the registry (focus applied), `false`
    /// otherwise (focus unchanged — caller decides next steps).
    pub fn set_focus(&mut self, id: ElementId) -> bool {
        if self.entries.iter().any(|e| e.id == id) {
            self.focused = Some(id);
            true
        } else {
            false
        }
    }

    /// Clear `focused` to `None`.
    pub fn clear_focus(&mut self) {
        self.focused = None;
    }

    /// Current focused element id, if any.
    pub fn focused(&self) -> Option<ElementId> {
        self.focused
    }

    /// True if `id` is currently registered (any tab_index).
    pub fn is_focusable(&self, id: ElementId) -> bool {
        self.entries.iter().any(|e| e.id == id)
    }

    /// True if `id` is registered AND part of the Tab cycle (tab_index >= 0).
    pub fn is_tab_reachable(&self, id: ElementId) -> bool {
        self.entries
            .iter()
            .any(|e| e.id == id && e.tab_index >= 0)
    }

    /// Look up an entry by id.
    pub fn entry(&self, id: ElementId) -> Option<&FocusableEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Move focus to the next tab-reachable element. Returns new focused id
    /// (does NOT call `set_focus` internally — caller applies).
    ///
    /// When `focused` is `None`, returns the first tab-reachable entry.
    /// Wraps at the end of the sorted list.
    pub fn shift_forward(&mut self) -> Option<ElementId> {
        self.shift(ShiftDir::Forward)
    }

    /// Move focus to the previous tab-reachable element. Returns new focused id.
    ///
    /// When `focused` is `None`, returns the last tab-reachable entry.
    /// Wraps at the start of the sorted list.
    pub fn shift_backward(&mut self) -> Option<ElementId> {
        self.shift(ShiftDir::Backward)
    }

    /// Clear `focused` if its element is no longer registered. Called by
    /// AppState after the prepaint walk re-populates the registry.
    pub fn prune_missing(&mut self) {
        if let Some(id) = self.focused
            && !self.is_focusable(id)
        {
            self.focused = None;
        }
    }

    /// Sort `sorted_indices` by `(tab_index, registration_index)` if `dirty`.
    fn ensure_sorted(&mut self) {
        if !self.dirty {
            return;
        }
        self.sorted_indices.clear();
        self.sorted_indices.extend(0..self.entries.len());
        // Stable sort by tab_index; equal tab_index falls back to registration order
        // (`sort_by_key` is stable, and the secondary key is the index itself).
        self.sorted_indices
            .sort_by_key(|&i| self.entries[i].tab_index);
        self.dirty = false;
    }

    fn shift(&mut self, dir: ShiftDir) -> Option<ElementId> {
        self.ensure_sorted();

        // Collect tab-reachable positions in sorted order. This is O(n) over the
        // (small) entries vec — no heap allocation beyond `tab_reachable`
        // (which lives on the stack as a SmallVec? — keep it simple: a Vec
        // here is only built per Tab keystroke, not per frame).
        let tab_reachable: Vec<usize> = self
            .sorted_indices
            .iter()
            .copied()
            .filter(|&i| self.entries[i].tab_index >= 0)
            .collect();

        if tab_reachable.is_empty() {
            return None;
        }

        let cur_pos = self.focused.and_then(|id| {
            tab_reachable
                .iter()
                .position(|&i| self.entries[i].id == id)
        });

        let next_idx = match (cur_pos, dir) {
            (None, ShiftDir::Forward) => tab_reachable[0],
            (None, ShiftDir::Backward) => *tab_reachable.last().unwrap(),
            (Some(p), ShiftDir::Forward) => tab_reachable[(p + 1) % tab_reachable.len()],
            (Some(p), ShiftDir::Backward) => {
                tab_reachable[(p + tab_reachable.len() - 1) % tab_reachable.len()]
            }
        };

        Some(self.entries[next_idx].id)
    }
}

impl Default for FocusRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FocusRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FocusRegistry")
            .field("entries", &self.entries)
            .field("focused", &self.focused)
            .field("dirty", &self.dirty)
            .finish()
    }
}

#[derive(Copy, Clone, Debug)]
enum ShiftDir {
    Forward,
    Backward,
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn empty_registry_has_no_focus_and_no_shift() {
        let mut r = FocusRegistry::new();
        assert_eq!(r.focused(), None);
        assert_eq!(r.shift_forward(), None);
        assert_eq!(r.shift_backward(), None);
        assert!(!r.is_focusable(id(1)));
    }

    #[test]
    fn single_entry_set_focus() {
        let mut r = FocusRegistry::new();
        r.register(entry(1, 0));
        assert!(r.set_focus(id(1)));
        assert_eq!(r.focused(), Some(id(1)));
    }

    #[test]
    fn set_focus_missing_returns_false() {
        let mut r = FocusRegistry::new();
        r.register(entry(1, 0));
        assert!(!r.set_focus(id(99)));
        assert_eq!(r.focused(), None);
    }

    #[test]
    fn clear_focus_sets_none() {
        let mut r = FocusRegistry::new();
        r.register(entry(1, 0));
        r.set_focus(id(1));
        r.clear_focus();
        assert_eq!(r.focused(), None);
    }
}
