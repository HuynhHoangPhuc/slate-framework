//! Focus management.
//!
//! Pure-data layer: types + tab-order traversal. No dispatch wiring lives here;
//! `AppState` owns a `RefCell<FocusRegistry>` and calls into this module from
//! its event-dispatch pipeline.
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

/// A registered focusable plus the modal trap scope it was registered under.
///
/// `trap_scope` is the [`ElementId`] of the nearest enclosing **modal** Overlay
/// (set from the registration-time scope stack), or `None` for entries outside
/// any modal. It is registry-internal — callers still construct the public
/// [`FocusableEntry`]; the scope is attached in [`FocusRegistry::register`].
#[derive(Copy, Clone, Debug)]
struct Focusable {
    entry: FocusableEntry,
    trap_scope: Option<ElementId>,
}

/// Owns the per-frame focusable list plus the current focused id.
///
/// Built up each prepaint via [`register`](Self::register) calls; consumed by
/// the keyboard dispatch path and the focus-ring paint pass.
///
/// # Modal focus trap
///
/// A modal [`Overlay`](crate::Overlay) brackets its content prepaint with
/// [`enter_trap_scope`](Self::enter_trap_scope) / [`exit_trap_scope`](Self::exit_trap_scope),
/// so every focusable registered inside it is tagged with the overlay's id. While
/// a modal is open, [`active_trap`](Self::active_trap_scope) is the **deepest**
/// open modal and Tab cycling ([`shift_forward`](Self::shift_forward) /
/// [`shift_backward`](Self::shift_backward)) is filtered to that scope — focus
/// cannot escape the modal. With no modal open the filter is inert and Tab walks
/// the whole tree as before.
pub struct FocusRegistry {
    entries: Vec<Focusable>,
    focused: Option<ElementId>,
    /// True when `entries` has changed since the last sort.
    dirty: bool,
    /// Indices into `entries`, stable-sorted by `(tab_index, registration_index)`.
    /// Includes negative-tab_index entries; Tab traversal filters them out.
    sorted_indices: Vec<usize>,
    /// Registration-time stack of modal trap scopes (top = current). Pushed by
    /// [`enter_trap_scope`](Self::enter_trap_scope) before a modal's content is
    /// prepainted, popped after; each [`register`](Self::register) tags its
    /// entry with the top of this stack.
    scope_stack: Vec<ElementId>,
    /// Every modal scope entered this frame, with its z-depth. Drives the focus
    /// lifecycle reconcile (auto-focus on open / restore on close).
    modal_scopes: Vec<(ElementId, i32)>,
    /// The trap enforced on Tab: the deepest (highest-depth) modal open this
    /// frame, or `None` when no modal is open. Set during registration, read
    /// after the prepaint walk; reset by [`clear`](Self::clear).
    active_trap: Option<(ElementId, i32)>,
}

impl FocusRegistry {
    /// Create an empty registry with no focused element.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            focused: None,
            dirty: false,
            sorted_indices: Vec::new(),
            scope_stack: Vec::new(),
            modal_scopes: Vec::new(),
            active_trap: None,
        }
    }

    /// Clear all entries + per-frame trap state (call at start of every prepaint).
    ///
    /// Does NOT clear `focused` — call [`prune_missing`](Self::prune_missing)
    /// after re-registration to auto-clear if focused was unmounted.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.sorted_indices.clear();
        self.dirty = false;
        self.scope_stack.clear();
        self.modal_scopes.clear();
        self.active_trap = None;
    }

    /// Register a focusable element during prepaint, tagged with the current
    /// modal trap scope (top of the scope stack, if any).
    pub fn register(&mut self, entry: FocusableEntry) {
        let trap_scope = self.scope_stack.last().copied();
        self.entries.push(Focusable { entry, trap_scope });
        self.dirty = true;
    }

    /// Begin a modal trap scope: focusables registered until the matching
    /// [`exit_trap_scope`](Self::exit_trap_scope) are tagged with `scope` and
    /// become the only Tab-reachable entries while this is the deepest open
    /// modal. Called by a modal [`Overlay`](crate::Overlay) around its content.
    ///
    /// `depth` is the overlay's z-depth; the deepest (highest-depth) modal open
    /// this frame wins the trap (ties: last entered wins).
    pub fn enter_trap_scope(&mut self, scope: ElementId, depth: i32) {
        self.scope_stack.push(scope);
        self.modal_scopes.push((scope, depth));
        match self.active_trap {
            Some((_, d)) if depth < d => {}
            _ => self.active_trap = Some((scope, depth)),
        }
    }

    /// End the current modal trap scope (the `active_trap` itself persists for
    /// the frame — it is the post-walk enforcement target).
    pub fn exit_trap_scope(&mut self) {
        self.scope_stack.pop();
    }

    /// The modal scope Tab is currently trapped within, if any.
    pub fn active_trap_scope(&self) -> Option<ElementId> {
        self.active_trap.map(|(id, _)| id)
    }

    /// Set `focused` to `id` if `id` is registered.
    ///
    /// Returns `true` when `id` is in the registry (focus applied), `false`
    /// otherwise (focus unchanged — caller decides next steps).
    pub fn set_focus(&mut self, id: ElementId) -> bool {
        if self.entries.iter().any(|e| e.entry.id == id) {
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
        self.entries.iter().any(|e| e.entry.id == id)
    }

    /// True if `id` is registered AND part of the Tab cycle (tab_index >= 0).
    pub fn is_tab_reachable(&self, id: ElementId) -> bool {
        self.entries
            .iter()
            .any(|e| e.entry.id == id && e.entry.tab_index >= 0)
    }

    /// Look up an entry by id.
    pub fn entry(&self, id: ElementId) -> Option<&FocusableEntry> {
        self.entries
            .iter()
            .find(|e| e.entry.id == id)
            .map(|e| &e.entry)
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
            .sort_by_key(|&i| self.entries[i].entry.tab_index);
        self.dirty = false;
    }

    fn shift(&mut self, dir: ShiftDir) -> Option<ElementId> {
        self.ensure_sorted();

        // While a modal is open, Tab is trapped to its scope: only entries
        // tagged with the active trap are reachable. Inert when no modal open.
        let trap = self.active_trap.map(|(id, _)| id);

        // Collect tab-reachable positions in sorted order. This is O(n) over the
        // (small) entries vec — no heap allocation beyond `tab_reachable`
        // (which lives on the stack as a SmallVec? — keep it simple: a Vec
        // here is only built per Tab keystroke, not per frame).
        let tab_reachable: Vec<usize> = self
            .sorted_indices
            .iter()
            .copied()
            .filter(|&i| self.entries[i].entry.tab_index >= 0)
            .filter(|&i| match trap {
                Some(t) => self.entries[i].trap_scope == Some(t),
                None => true,
            })
            .collect();

        if tab_reachable.is_empty() {
            return None;
        }

        let cur_pos = self
            .focused
            .and_then(|id| tab_reachable.iter().position(|&i| self.entries[i].entry.id == id));

        let next_idx = match (cur_pos, dir) {
            (None, ShiftDir::Forward) => tab_reachable[0],
            (None, ShiftDir::Backward) => *tab_reachable.last().unwrap(),
            (Some(p), ShiftDir::Forward) => tab_reachable[(p + 1) % tab_reachable.len()],
            (Some(p), ShiftDir::Backward) => {
                tab_reachable[(p + tab_reachable.len() - 1) % tab_reachable.len()]
            }
        };

        Some(self.entries[next_idx].entry.id)
    }

    /// First Tab-reachable focusable tagged with modal scope `scope`, in tab
    /// order. Used to auto-focus into a modal when it opens.
    pub fn first_in_scope(&mut self, scope: ElementId) -> Option<ElementId> {
        self.ensure_sorted();
        self.sorted_indices
            .iter()
            .copied()
            .filter(|&i| self.entries[i].entry.tab_index >= 0)
            .find(|&i| self.entries[i].trap_scope == Some(scope))
            .map(|i| self.entries[i].entry.id)
    }

    /// Reconcile the modal focus lifecycle against `stack`, the caller-owned
    /// record of currently-open modals as `(modal_id, focus_to_restore)`.
    ///
    /// Run once per frame after the prepaint walk (and after
    /// [`prune_missing`](Self::prune_missing)):
    /// - **Close**: drop **every** stacked modal no longer present (so it can
    ///   reopen cleanly later). Restore focus only once *no* modal remains open,
    ///   to the focus saved by the earliest-opened modal that closed (the
    ///   pre-modal focus). While any modal is still open, focus is left to it —
    ///   it owns the trap. The restore target is skipped if it has unmounted.
    /// - **Open**: for each newly-present modal (shallowest first), save the
    ///   current focus and auto-focus the modal's first focusable.
    ///
    /// A modal present every frame between open and close is left untouched, so
    /// focus the user moves *within* the modal is preserved until it closes.
    /// Strict-nested modals close LIFO as a special case of the above; the
    /// general drain also handles out-of-order sibling modals correctly.
    pub fn reconcile_modal_focus(&mut self, stack: &mut Vec<(ElementId, Option<ElementId>)>) {
        // Present modals, shallowest-depth first = open order.
        let mut present = self.modal_scopes.clone();
        present.sort_by_key(|&(_, d)| d);
        let present_ids: Vec<ElementId> = present.iter().map(|&(id, _)| id).collect();

        // CLOSES: drain all stack entries whose modal vanished. `stack` is
        // depth-ascending, so the first drained entry is the shallowest (earliest
        // opened) closed modal — its saved focus is the pre-modal focus to
        // restore once everything is closed.
        let mut earliest_restore: Option<Option<ElementId>> = None;
        stack.retain(|&(id, prev)| {
            let present = present_ids.contains(&id);
            if !present && earliest_restore.is_none() {
                earliest_restore = Some(prev);
            }
            present
        });
        // Only restore when no modal remains open; otherwise the still-open
        // (deepest) modal keeps the focus trap.
        if stack.is_empty()
            && let Some(Some(r)) = earliest_restore
            && self.is_focusable(r)
        {
            self.set_focus(r);
        }

        // OPENS: a newly-present modal saves prior focus + auto-focuses its first
        // focusable.
        for (id, _) in present {
            if stack.iter().any(|&(s, _)| s == id) {
                continue;
            }
            let prev = self.focused;
            if let Some(first) = self.first_in_scope(id) {
                self.set_focus(first);
            }
            stack.push((id, prev));
        }
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

    // ----- Modal focus trap -----

    /// Register two outside focusables, then a modal scope (id 100) with two
    /// more inside. Mirrors a prepaint walk where a modal Overlay brackets its
    /// content.
    fn registry_with_modal() -> FocusRegistry {
        let mut r = FocusRegistry::new();
        r.register(entry(1, 0)); // outside
        r.register(entry(2, 0)); // outside
        r.enter_trap_scope(id(100), 1000);
        r.register(entry(10, 0)); // inside modal
        r.register(entry(11, 0)); // inside modal
        r.exit_trap_scope();
        r
    }

    #[test]
    fn no_modal_tab_cycles_whole_tree() {
        let mut r = FocusRegistry::new();
        r.register(entry(1, 0));
        r.register(entry(2, 0));
        assert_eq!(r.active_trap_scope(), None);
        assert_eq!(r.shift_forward(), Some(id(1)));
        r.set_focus(id(1));
        assert_eq!(r.shift_forward(), Some(id(2)));
        r.set_focus(id(2));
        assert_eq!(r.shift_forward(), Some(id(1)), "wraps across whole tree");
    }

    #[test]
    fn modal_trap_cycles_only_inside_scope() {
        let mut r = registry_with_modal();
        assert_eq!(r.active_trap_scope(), Some(id(100)));
        // From no focus, Tab enters the modal's first entry, not the tree's.
        assert_eq!(r.shift_forward(), Some(id(10)));
        r.set_focus(id(10));
        assert_eq!(r.shift_forward(), Some(id(11)));
        r.set_focus(id(11));
        // Wraps back inside the modal — never escapes to id(1)/id(2).
        assert_eq!(r.shift_forward(), Some(id(10)));
    }

    #[test]
    fn modal_trap_backward_stays_inside_scope() {
        let mut r = registry_with_modal();
        r.set_focus(id(10));
        assert_eq!(r.shift_backward(), Some(id(11)), "wraps to last inside");
    }

    #[test]
    fn trap_ignores_focus_currently_outside_scope() {
        // Focus sits on an outside element; Tab still jumps into the modal.
        let mut r = registry_with_modal();
        r.set_focus(id(1));
        assert_eq!(r.shift_forward(), Some(id(10)));
    }

    #[test]
    fn deepest_modal_wins_the_trap() {
        let mut r = FocusRegistry::new();
        r.register(entry(1, 0));
        r.enter_trap_scope(id(100), 1000);
        r.register(entry(10, 0));
        r.enter_trap_scope(id(200), 2000); // nested, deeper
        r.register(entry(20, 0));
        r.exit_trap_scope();
        r.exit_trap_scope();
        assert_eq!(r.active_trap_scope(), Some(id(200)));
        assert_eq!(r.shift_forward(), Some(id(20)), "only deepest reachable");
    }

    #[test]
    fn first_in_scope_returns_first_tab_reachable() {
        let mut r = registry_with_modal();
        assert_eq!(r.first_in_scope(id(100)), Some(id(10)));
        assert_eq!(r.first_in_scope(id(999)), None);
    }

    #[test]
    fn first_in_scope_skips_negative_tab_index() {
        let mut r = FocusRegistry::new();
        r.enter_trap_scope(id(100), 1000);
        r.register(entry(10, -1)); // programmatic-only, not Tab-reachable
        r.register(entry(11, 0));
        r.exit_trap_scope();
        assert_eq!(r.first_in_scope(id(100)), Some(id(11)));
    }

    // ----- Modal focus lifecycle (reconcile) -----

    #[test]
    fn reconcile_open_saves_prev_and_autofocuses_first() {
        let mut r = registry_with_modal();
        r.set_focus(id(1)); // outside focus before the modal opened
        let mut stack = Vec::new();
        r.reconcile_modal_focus(&mut stack);
        assert_eq!(r.focused(), Some(id(10)), "auto-focus modal's first");
        assert_eq!(stack, vec![(id(100), Some(id(1)))], "saved prior focus");
    }

    #[test]
    fn reconcile_close_restores_prev() {
        // Frame 1: modal open.
        let mut r = registry_with_modal();
        r.set_focus(id(1));
        let mut stack = Vec::new();
        r.reconcile_modal_focus(&mut stack);
        assert_eq!(r.focused(), Some(id(10)));

        // Frame 2: modal gone; the outside elements are still present.
        let mut r2 = FocusRegistry::new();
        r2.register(entry(1, 0));
        r2.register(entry(2, 0));
        r2.set_focus(id(10)); // stale (would be pruned in the real loop)
        r2.reconcile_modal_focus(&mut stack);
        assert_eq!(r2.focused(), Some(id(1)), "prior focus restored");
        assert!(stack.is_empty(), "closed modal popped");
    }

    #[test]
    fn reconcile_keeps_focus_while_modal_stays_open() {
        let mut r = registry_with_modal();
        r.set_focus(id(1));
        let mut stack = Vec::new();
        r.reconcile_modal_focus(&mut stack); // opens, focus -> id(10)

        // Frame 2: modal still open, user Tabbed to id(11) within it.
        let mut r2 = registry_with_modal();
        r2.set_focus(id(11));
        r2.reconcile_modal_focus(&mut stack);
        assert_eq!(r2.focused(), Some(id(11)), "in-modal focus untouched");
        assert_eq!(stack, vec![(id(100), Some(id(1)))], "stack unchanged");
    }

    #[test]
    fn reconcile_sibling_close_under_open_modal_drains_and_holds_focus() {
        // Two independent sibling modals: A (depth 100), B (depth 200) both open,
        // then A closes while B stays open. A must be dropped from the stack (so
        // it can reopen later) and focus must stay inside B — not jump to A's
        // restore target while B still traps.
        let mut stack = vec![(id(1), Some(id(50))), (id(2), Some(id(51)))];

        let mut r = FocusRegistry::new();
        r.register(entry(51, 0)); // B's auto-focused element, still present
        r.enter_trap_scope(id(2), 200); // only B present this frame
        r.register(entry(60, 0));
        r.exit_trap_scope();
        r.set_focus(id(60));
        r.reconcile_modal_focus(&mut stack);

        assert_eq!(stack, vec![(id(2), Some(id(51)))], "closed sibling A drained");
        assert_eq!(r.focused(), Some(id(60)), "focus stays inside still-open B");
    }

    #[test]
    fn reconcile_all_modals_closed_restores_earliest_prev() {
        // A then B opened; both close the same frame. Focus restores to the
        // pre-modal focus saved by the earliest (shallowest) modal, A.
        let mut stack = vec![(id(1), Some(id(50))), (id(2), Some(id(51)))];

        let mut r = FocusRegistry::new();
        r.register(entry(50, 0)); // A's saved pre-modal focus, still present
        r.register(entry(51, 0));
        r.reconcile_modal_focus(&mut stack);

        assert!(stack.is_empty(), "both modals drained");
        assert_eq!(r.focused(), Some(id(50)), "restored earliest pre-modal focus");
    }

    #[test]
    fn reconcile_restore_skips_vanished_prev() {
        let mut r = registry_with_modal();
        r.set_focus(id(1));
        let mut stack = Vec::new();
        r.reconcile_modal_focus(&mut stack);

        // Frame 2: modal closed AND the prior-focus element is also gone.
        let mut r2 = FocusRegistry::new();
        r2.register(entry(2, 0)); // id(1) no longer present
        r2.reconcile_modal_focus(&mut stack);
        assert_eq!(r2.focused(), None, "no restore when prior focus vanished");
        assert!(stack.is_empty());
    }
}
