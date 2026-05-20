//! TextField undo / redo history — Apple Notes-style coalesce.
//!
//! `UndoStack` keeps a bounded history of **post-edit** snapshots. Typing
//! within 500 ms of the previous keystroke folds into the same undo step;
//! motion, time gaps, op-type changes, and discrete ops (paste / cut / IME
//! commit) open a fresh step. The stack is owned by the render-surviving
//! `ImeState` so history persists across the per-frame `TextField` rebuild.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

const UNDO_STACK_CAP: usize = 100;
const COALESCE_TIME_GAP: Duration = Duration::from_millis(500);

/// Snapshot of TextField state. Stored as **post-edit** snapshots — the state
/// the field was in immediately after the edit that produced it. `entries[0]`
/// is the baseline (initial value) so undo can always walk back to "before the
/// first edit".
#[derive(Clone, Debug)]
pub(crate) struct EditSnapshot {
    pub(crate) text: String,
    pub(crate) caret: usize,
    pub(crate) anchor: Option<usize>,
}

/// Classification of an edit for coalesce decisions. `Insert` and `Backspace`
/// are coalescible bursts; `Discrete` (paste, cut, selection-delete, IME
/// commit) always pushes a fresh undo step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditOp {
    Insert,
    Backspace,
    Discrete,
}

/// Apple Notes-style coalesce: typing within 500 ms of the previous typing
/// keystroke folds into the same undo step; anything else (motion, time gap,
/// op-type change, discrete op) opens a new step.
/// Opaque to external crates: fields are private and constructors / mutators
/// are `pub(crate)`, so the only thing downstream code can do with the `pub
/// undo` field on [`ImeState`](crate::ime::ImeState) is move or clone it. The
/// type is `pub` solely so that public field can name it without a privacy leak.
#[derive(Clone, Debug)]
pub struct UndoStack {
    /// History of post-edit states. Always non-empty; `entries[0]` is the
    /// baseline. `entries[cursor]` reflects the live document.
    entries: VecDeque<EditSnapshot>,
    cursor: usize,
    /// Op that produced the head entry. `None` after undo/redo so the next
    /// edit pushes fresh rather than coalescing into a state the user already
    /// walked back through.
    head_op: Option<EditOp>,
    head_edit_t: Option<Instant>,
    /// Set by non-typing caret motion (arrow / Home / End / click / drag).
    /// Forces the next edit to open a fresh undo step.
    motion_since_last_edit: bool,
}

impl Default for UndoStack {
    /// Empty stack seeded with an empty baseline — equivalent to a
    /// default-constructed TextField (no text, caret at 0). Lets `ImeState`
    /// derive `Default`; the real baseline is re-seeded once in
    /// `TextField::prepaint`.
    fn default() -> Self {
        Self::with_baseline(String::new(), 0)
    }
}

impl UndoStack {
    /// Construct an undo stack seeded with the field's initial state.
    pub(crate) fn with_baseline(text: String, caret: usize) -> Self {
        let mut entries = VecDeque::with_capacity(UNDO_STACK_CAP);
        entries.push_back(EditSnapshot {
            text,
            caret,
            anchor: None,
        });
        Self {
            entries,
            cursor: 0,
            head_op: None,
            head_edit_t: None,
            motion_since_last_edit: false,
        }
    }

    /// Record a completed edit. `post` is the state *after* the mutation. The
    /// 4-trigger policy decides whether to push a new entry or overwrite the
    /// head entry in place.
    pub(crate) fn record_edit(&mut self, op: EditOp, post: EditSnapshot) {
        // New edits truncate the redo tail.
        if self.cursor + 1 < self.entries.len() {
            self.entries.drain(self.cursor + 1..);
        }

        if self.should_push(op) {
            if self.entries.len() == UNDO_STACK_CAP {
                self.entries.pop_front();
                self.cursor = self.cursor.saturating_sub(1);
            }
            self.entries.push_back(post);
            self.cursor = self.entries.len() - 1;
        } else if let Some(slot) = self.entries.get_mut(self.cursor) {
            *slot = post;
        }

        self.head_op = Some(op);
        self.head_edit_t = Some(Instant::now());
        self.motion_since_last_edit = false;
    }

    /// Mark that the caret moved without an edit. Forces the next edit to
    /// open a fresh undo step.
    pub(crate) fn mark_motion(&mut self) {
        self.motion_since_last_edit = true;
    }

    /// Pop one undo step. Returns the state to restore (and the field's live
    /// state should be assigned from it). `None` when nothing left to undo.
    pub(crate) fn undo(&mut self) -> Option<EditSnapshot> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        // After undo, the head op is no longer the live op — the next edit
        // must open a new step, never coalesce with what the user walked back.
        self.head_op = None;
        self.head_edit_t = None;
        self.motion_since_last_edit = false;
        Some(self.entries[self.cursor].clone())
    }

    /// Step forward through redo. Returns the state to restore, or `None` if
    /// nothing to redo.
    pub(crate) fn redo(&mut self) -> Option<EditSnapshot> {
        if self.cursor + 1 >= self.entries.len() {
            return None;
        }
        self.cursor += 1;
        self.head_op = None;
        self.head_edit_t = None;
        self.motion_since_last_edit = false;
        Some(self.entries[self.cursor].clone())
    }

    fn should_push(&self, op: EditOp) -> bool {
        let Some(prev_op) = self.head_op else {
            return true;
        };
        // Trigger 4: discrete ops never coalesce (in either direction).
        if op == EditOp::Discrete || prev_op == EditOp::Discrete {
            return true;
        }
        // Trigger 3: op-type change.
        if prev_op != op {
            return true;
        }
        // Trigger 1: time gap > 500 ms since last edit.
        if let Some(t) = self.head_edit_t
            && t.elapsed() > COALESCE_TIME_GAP
        {
            return true;
        }
        // Trigger 2: caret moved by non-typing action since last edit.
        if self.motion_since_last_edit {
            return true;
        }
        false
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(crate) fn cursor(&self) -> usize {
        self.cursor
    }
}

// ---------------------------------------------------------------------------
// Unit tests — UndoStack coalesce policy
// ---------------------------------------------------------------------------

#[cfg(test)]
mod undo_tests {
    use super::*;

    fn snap(text: &str, caret: usize) -> EditSnapshot {
        EditSnapshot {
            text: text.to_string(),
            caret,
            anchor: None,
        }
    }

    fn fresh() -> UndoStack {
        UndoStack::with_baseline(String::new(), 0)
    }

    #[test]
    fn baseline_is_present_and_undo_returns_none() {
        let mut s = fresh();
        assert_eq!(s.len(), 1);
        assert_eq!(s.cursor(), 0);
        assert!(s.undo().is_none());
    }

    #[test]
    fn typing_burst_coalesces_into_single_step() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("a", 1));
        s.record_edit(EditOp::Insert, snap("ab", 2));
        s.record_edit(EditOp::Insert, snap("abc", 3));
        // Baseline + one coalesced typing entry.
        assert_eq!(s.len(), 2);
        let restored = s.undo().expect("undo should restore baseline");
        assert_eq!(restored.text, "");
    }

    #[test]
    fn op_type_change_opens_new_step() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("a", 1));
        s.record_edit(EditOp::Backspace, snap("", 0));
        // Baseline + insert + backspace = 3.
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn motion_marks_split_typing_into_two_steps() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("a", 1));
        s.mark_motion();
        s.record_edit(EditOp::Insert, snap("ab", 2));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn discrete_op_never_coalesces() {
        let mut s = fresh();
        s.record_edit(EditOp::Discrete, snap("p", 1));
        s.record_edit(EditOp::Discrete, snap("pp", 2));
        // Two discrete ops in a row both push.
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn typing_after_discrete_op_does_not_coalesce_backward() {
        let mut s = fresh();
        s.record_edit(EditOp::Discrete, snap("pasted", 6));
        s.record_edit(EditOp::Insert, snap("pastedX", 7));
        assert_eq!(s.len(), 3);
    }

    #[test]
    fn undo_then_edit_truncates_redo_tail() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("a", 1));
        s.record_edit(EditOp::Backspace, snap("", 0));
        s.undo();
        // cursor now at the insert entry; new edit truncates redo tail.
        s.record_edit(EditOp::Insert, snap("aX", 2));
        assert_eq!(s.len(), 3);
        assert_eq!(s.cursor(), 2);
        assert!(s.redo().is_none(), "redo tail must be empty after new edit");
    }

    #[test]
    fn redo_walks_forward_after_undo() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("a", 1));
        s.record_edit(EditOp::Backspace, snap("", 0));
        let _ = s.undo();
        let redone = s.redo().expect("redo available");
        assert_eq!(redone.text, "");
    }

    #[test]
    fn capacity_overflow_drops_oldest_and_clamps_cursor() {
        let mut s = fresh();
        // Force a fresh push every step (motion between Inserts).
        for i in 1..=(UNDO_STACK_CAP + 5) {
            s.record_edit(EditOp::Insert, snap(&"x".repeat(i), i));
            s.mark_motion();
        }
        assert_eq!(s.len(), UNDO_STACK_CAP);
        // cursor points at the latest entry — never out of range.
        assert!(s.cursor() < s.len());
    }

    #[test]
    fn undo_after_typing_burst_restores_pre_burst_state() {
        let mut s = fresh();
        s.record_edit(EditOp::Insert, snap("h", 1));
        s.record_edit(EditOp::Insert, snap("he", 2));
        s.record_edit(EditOp::Insert, snap("hel", 3));
        let restored = s.undo().expect("undo");
        assert_eq!(restored.text, "", "Apple Notes: one undo erases the burst");
    }
}
