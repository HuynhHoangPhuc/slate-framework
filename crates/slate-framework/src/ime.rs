//! IME state registry + per-element composition state.
//!
//! Phase 9c. Mirrors the [`FocusRegistry`](crate::focus::FocusRegistry) lifecycle:
//! cleared at the start of every prepaint, re-registered per ime-capable
//! element during the prepaint walk, then [`prune_missing`](ImeRegistry::prune_missing)
//! collapses any state belonging to unmounted elements.
//!
//! # Cache-then-query (ADR-001 amendment)
//!
//! The platform `WindowImeDelegate` query channel does NOT traverse this
//! registry directly. Instead, `AppState` republishes a snapshot
//! ([`CachedImeQuery`]) at deterministic points (end of `dispatch_ime_preedit`,
//! end of every paint), and the delegate methods read that cache only. The
//! registry is the source of truth; the cache is the safe read path.
//!
//! # Per-element state
//!
//! Each ime-capable element owns one [`ImeState`] held inside an `Rc<RefCell<_>>`
//! so per-element handlers can mutate it without re-borrowing the registry
//! while the dispatch loop holds a registry-level borrow.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use slate_platform::{PhysicalRect, WindowId};

use crate::elements::text_edit::undo::UndoStack;
use crate::types::ElementId;

// ---------------------------------------------------------------------------
// Preedit (composition payload)
// ---------------------------------------------------------------------------

/// In-flight composition. Empty preedit ≡ `None` (the framework never holds a
/// zero-length `Preedit`; callers clear to `None` instead).
#[derive(Clone, Debug, Default)]
pub struct Preedit {
    /// Composition text (UTF-8). Replaces any prior preedit on each
    /// `ImePreedit` event.
    pub text: String,
    /// UTF-8 byte offset into `text` of the IME caret (where the next
    /// keystroke would land).
    pub cursor_byte_offset: usize,
    /// IME-highlighted target-converted range, UTF-8 byte range into `text`.
    /// `None` if the OS provided no highlight (e.g. macOS Roman, raw mode).
    pub selection: Option<Range<usize>>,
}

// ---------------------------------------------------------------------------
// BlinkState (caret blink)
// ---------------------------------------------------------------------------

/// Blink state for the editable-text caret. Lives on `ImeState` so handlers
/// reading the per-element `RefCell<ImeState>` can reset the cycle in the same
/// borrow they use for caret motion. The 530 ms cycle matches macOS
/// `NSTextInsertionPointBlinkPeriod`.
#[derive(Clone, Debug)]
pub struct BlinkState {
    /// Whether the caret is currently shown. `paint()` checks this each frame
    /// while focused; reset to `true` on every caret-affecting input.
    pub visible: bool,
    /// Instant at which the caret should next toggle. `None` while the field
    /// is unfocused or has not painted yet — paint arms the timer on first
    /// focused frame.
    pub next: Option<Instant>,
}

impl Default for BlinkState {
    fn default() -> Self {
        Self {
            visible: true,
            next: None,
        }
    }
}

// ---------------------------------------------------------------------------
// ImeState (per-element)
// ---------------------------------------------------------------------------

/// Per-element IME state: the committed buffer, caret, and any active
/// composition.
///
/// Default constructs an empty state (no preedit, caret 0, zero rect). The
/// `caret_client_rect` is updated by the host element during paint and at the
/// end of `dispatch_ime_preedit`; the cached query path reads from it.
#[derive(Clone, Debug, Default)]
pub struct ImeState {
    /// Committed text buffer (TextField source-of-truth).
    pub text: String,
    /// Byte offset into `text` of the caret.
    pub caret: usize,
    /// Which side of a direction boundary the caret binds to when one logical
    /// byte maps to two visual x-positions on a mixed-direction line. Inert
    /// (`Downstream`) on pure-LTR / CJK text. Set by visual ←/→ motion as it
    /// crosses an LTR↔RTL seam; consumed by the caret-x paint path.
    pub caret_affinity: slate_text::Affinity,
    /// Anchor end of a non-empty selection, as a byte offset into `text`. The
    /// selected range is `min(caret, anchor) .. max(caret, anchor)`; `None`
    /// means caret-only (no selection).
    ///
    /// Selection state lives alongside the IME composition fields here for
    /// framework-internal cohesion (single struct, single registry slot, no
    /// extra borrow churn around the dispatch loop). Once `TextArea` arrives
    /// the editor-state side should split out so this struct only carries
    /// what the platform IME contract actually requires.
    pub selection_anchor: Option<usize>,
    /// Active composition, if any. `None` outside of IME sessions.
    pub preedit: Option<Preedit>,
    /// Window-client-relative physical-pixel rect of the caret, top-left
    /// origin. `None` means no element has published a caret rect this frame
    /// (default state, or focused element forgot to publish during paint);
    /// `Some(rect)` is the current caret. Updated by the element during paint
    /// and consumed by the cache republish path. Platform consumers convert
    /// to the OS's expected space (macOS → screen; Windows → client coords
    /// as-is) and treat `None` as "no anchor available."
    pub caret_client_rect: Option<PhysicalRect>,
    // ----- TextField paint cache + interaction state (NOT IME contract) -----
    /// Most recent shaped line, cached during paint so that mouse handlers can
    /// invert pixel-x to byte offset without re-shaping (the dispatch path has
    /// no `TextSystem` access). At most one frame stale relative to `text`.
    ///
    /// Like `selection_anchor`, this rides on `ImeState` for framework-internal
    /// cohesion. The 10a.3 split-on-TextArea contract applies here too.
    pub last_shaped: Option<Rc<slate_text::ShapedLine>>,
    /// Logical-px x-origin of the element's painted bounds. Mouse handlers
    /// subtract this from the window-relative event position to get a
    /// line-relative x for `byte_at_pixel_x`. Updated during paint.
    pub paint_origin_x: f32,
    /// Logical-px y-origin of the element's painted bounds. Multi-line
    /// (`TextArea`) mouse handlers subtract this from the event position to get
    /// a line-relative y for the visual-line hit-test. Unused on the single-line
    /// path. Updated during paint.
    pub paint_origin_y: f32,
    /// True while a primary-button drag is in progress on this element. Set on
    /// `on_mouse_down`, cleared on `on_mouse_up`. Drives the "anchor on first
    /// drag move" rule in `on_mouse_move`.
    pub dragging: bool,
    /// Caret blink state. Reset by handlers on caret-affecting input; advanced
    /// by `paint()` while focused.
    pub blink: BlinkState,
    /// Undo / redo history for this element. Lives here (not on the per-frame
    /// `TextField`) so it survives the `value.set()`-driven re-render between
    /// every keystroke — `register` returns the same `Rc` each frame, so the
    /// history persists until the element unmounts (`prune_missing`).
    ///
    /// `UndoStack` is opaque to external crates (private fields, `pub(crate)`
    /// methods), so this `pub` field stays externally inert — it only keeps
    /// `ImeState` constructible by struct literal as before.
    pub undo: UndoStack,
    /// Whether the undo baseline has been seeded from the initial value. The
    /// one-time guard for `seed_undo_baseline`: `register` is get-or-insert and
    /// its `dirty` flag is registry-wide, so it cannot gate per-element seeding.
    pub undo_seeded: bool,
    // ----- Multi-line (TextArea) state — `None`/unused on the single-line path -----
    /// Sticky horizontal target for vertical caret motion (up/down). Set when a
    /// non-vertical motion moves the caret, consumed by up/down so a run of
    /// vertical moves tracks a single column. `None` on the single-line path.
    /// (Wired in a later phase; carried here so multi-line state lives in one
    /// struct.)
    pub desired_x: Option<f32>,
    /// Most recent multi-line layout, cached during paint so mouse/key handlers
    /// can map bytes↔(line, x) without re-shaping. `None` on the single-line
    /// path (TextField uses `last_shaped` instead).
    pub last_layout: Option<Rc<slate_text::MultilineLayout>>,
    /// Timestamp of the previous mouse-down, used to detect multi-click runs
    /// (double / triple click). `None` until the first click. Reset to the new
    /// click time on every mouse-down.
    pub last_click_time: Option<Instant>,
    /// Window-relative position of the previous mouse-down. A new click only
    /// extends the run when it lands within `DOUBLE_CLICK_DISTANCE` of this.
    pub last_click_pos: (f32, f32),
    /// Length of the current click run: 1 = single (caret), 2 = double (word),
    /// 3 = triple (line). Wraps back to 1 after a triple click so a fourth click
    /// starts a fresh run. Reset to 0 when a drag ends on a different element.
    pub click_count: u8,
}

impl ImeState {
    /// Seed the undo baseline from the current `text` / `caret`, exactly once
    /// per element. Re-running is a no-op once seeded, so the per-frame
    /// `TextField::prepaint` call never resets a stack that already holds
    /// recorded edits (re-seeding would reintroduce the lost-history bug).
    pub fn seed_undo_baseline(&mut self) {
        if !self.undo_seeded {
            self.undo = UndoStack::with_baseline(self.text.clone(), self.caret);
            self.undo_seeded = true;
        }
    }

    /// Answer `WindowImeDelegate::ime_text`. Returns the substring of `text`
    /// inside `range`, or `None` if `range` is out of bounds or splits a
    /// UTF-8 codepoint.
    pub fn answer_ime_text(&self, range: Range<usize>) -> Option<String> {
        self.text.get(range).map(|s| s.to_string())
    }

    /// Answer `WindowImeDelegate::ime_selected_range`. Returns
    /// `min(caret, anchor)..max(caret, anchor)` when a selection anchor is
    /// set; otherwise the caret-only collapsed range `caret..caret`.
    pub fn answer_selected_range(&self) -> Option<Range<usize>> {
        match self.selection_anchor {
            Some(anchor) => {
                let (lo, hi) = if self.caret <= anchor {
                    (self.caret, anchor)
                } else {
                    (anchor, self.caret)
                };
                Some(lo..hi)
            }
            None => Some(self.caret..self.caret),
        }
    }

    /// Answer `WindowImeDelegate::ime_marked_range`. Returns the preedit
    /// range relative to the committed buffer's caret position, or `None`
    /// when no composition is active.
    pub fn answer_marked_range(&self) -> Option<Range<usize>> {
        let preedit = self.preedit.as_ref()?;
        let start = self.caret;
        let end = start + preedit.text.len();
        Some(start..end)
    }
}

// ---------------------------------------------------------------------------
// ImeRegistry (per-frame)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// CachedImeQuery (read-side snapshot for WindowImeDelegate)
// ---------------------------------------------------------------------------

/// Snapshot of the focused element's `ImeState` consumed by the platform
/// `WindowImeDelegate` query channel. Republished at deterministic points
/// (end of `dispatch_ime_preedit`, end of every paint); never traversed
/// directly by the OS query callbacks.
///
/// All fields are `None` when no element claims IME this frame.
#[derive(Clone, Debug, Default)]
pub struct CachedImeQuery {
    /// Caret rect in window-client-relative physical pixels (top-left origin);
    /// platform consumers convert to the OS's expected space.
    pub caret_client_rect: Option<PhysicalRect>,
    /// Active composition byte range, if any.
    pub marked_range: Option<Range<usize>>,
    /// Current selection (caret-only collapses to empty range at caret).
    pub selected_range: Option<Range<usize>>,
    /// Bounded slice of the committed text near the caret. Tuple of
    /// `(absolute_range, text)` so the delegate can answer `ime_text`
    /// queries that fall inside the cached window without re-borrowing the
    /// registry.
    pub text_window: Option<(Range<usize>, String)>,
}

// ---------------------------------------------------------------------------
// PendingImeOp (deferred mutation queue)
// ---------------------------------------------------------------------------

/// Deferred IME mutation queued during dispatch. Drained by `AppState` after
/// the per-element handler chain unwinds, BEFORE `pending_focus_op` so a
/// Tab-during-composition lands its synthetic commit on the still-focused
/// element.
#[derive(Clone, Debug)]
pub enum PendingImeOp {
    /// Synthetic commit. Inserts `text` at the focused element's caret,
    /// advances the caret, and clears the preedit. Does NOT run user
    /// `on_ime_commit` handlers: this op is itself drained from within the
    /// dispatch unwind, so re-entering the handler chain here would risk
    /// re-entrant borrows of the same element state.
    Commit { window: WindowId, text: String },
}

// ---------------------------------------------------------------------------
// Display caret affinity
// ---------------------------------------------------------------------------

/// Caret affinity to use when resolving the display caret-x.
///
/// During an active IME composition the caret must bind [`Affinity::Downstream`]
/// regardless of the stored seam affinity: a preedit splices new text at the
/// caret, so the pre-composition Upstream/Downstream choice belongs to the
/// committed caret, not the preedit-shifted display caret. Re-applying the stale
/// value double-shifts the caret across an LTR↔RTL boundary. Outside composition
/// the stored affinity (set by visual ←/→ motion as it crosses a seam) is
/// authoritative.
///
/// Shared by the TextField and TextArea paint paths so the invariant lives in
/// one place — see [`crate::elements::text_field`] and
/// [`crate::elements::text_area`].
pub(crate) fn caret_affinity_for_display(
    has_preedit: bool,
    stored: slate_text::Affinity,
) -> slate_text::Affinity {
    if has_preedit {
        slate_text::Affinity::Downstream
    } else {
        stored
    }
}

/// Byte index the caret is painted at, given the display caret (the committed
/// caret, or the preedit's start byte during composition) and the active
/// preedit.
///
/// During composition the visible caret binds to the IME cursor *inside* the
/// preedit — where the next keystroke lands — not to the start of the composed
/// run. Without this the caret stays pinned at the composition start for the
/// whole session and only jumps once it commits, so it never visibly advances
/// as you type (most visible with multi-glyph CJK/Vietnamese composition). The
/// offset is clamped into the preedit text so a misreporting backend can't push
/// the caret past the composed run. Outside composition this is just the
/// display caret.
///
/// Shared by the TextField and TextArea paint paths so the invariant lives in
/// one place — see [`crate::elements::text_field`] and
/// [`crate::elements::text_area`].
pub(crate) fn caret_display_byte(display_caret: usize, preedit: Option<&Preedit>) -> usize {
    match preedit {
        Some(p) => display_caret + p.cursor_byte_offset.min(p.text.len()),
        None => display_caret,
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
    fn answer_ime_text_returns_substring() {
        let s = ImeState {
            text: "hello world".to_string(),
            ..Default::default()
        };
        assert_eq!(s.answer_ime_text(0..5), Some("hello".to_string()));
        assert_eq!(s.answer_ime_text(6..11), Some("world".to_string()));
        assert_eq!(s.answer_ime_text(0..100), None);
    }

    #[test]
    fn answer_selected_range_returns_caret_empty_range() {
        let s = ImeState {
            text: "abc".to_string(),
            caret: 2,
            ..Default::default()
        };
        assert_eq!(s.answer_selected_range(), Some(2..2));
    }

    #[test]
    fn answer_marked_range_none_when_no_preedit() {
        let s = ImeState::default();
        assert_eq!(s.answer_marked_range(), None);
    }

    #[test]
    fn selection_anchor_defaults_to_none() {
        let s = ImeState::default();
        assert!(s.selection_anchor.is_none());
    }

    #[test]
    fn answer_selected_range_with_anchor_forward() {
        let s = ImeState {
            text: "abcdef".to_string(),
            caret: 5,
            selection_anchor: Some(2),
            ..Default::default()
        };
        assert_eq!(s.answer_selected_range(), Some(2..5));
    }

    #[test]
    fn answer_selected_range_with_anchor_reverse() {
        // Drag right-to-left: anchor > caret.
        let s = ImeState {
            text: "abcdef".to_string(),
            caret: 1,
            selection_anchor: Some(4),
            ..Default::default()
        };
        assert_eq!(s.answer_selected_range(), Some(1..4));
    }

    #[test]
    fn answer_selected_range_collapsed_anchor() {
        // anchor == caret → empty range at caret.
        let s = ImeState {
            text: "abc".to_string(),
            caret: 2,
            selection_anchor: Some(2),
            ..Default::default()
        };
        assert_eq!(s.answer_selected_range(), Some(2..2));
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

    #[test]
    fn seed_undo_baseline_is_one_time() {
        use crate::elements::text_edit::undo::{EditOp, EditSnapshot};
        let mut state = ImeState {
            text: "init".to_string(),
            caret: 4,
            ..Default::default()
        };
        state.seed_undo_baseline();
        assert!(state.undo_seeded);
        // Record an edit, then re-run seeding (simulating a later frame). The
        // guard must keep the stack intact — re-seeding would drop the history.
        state.undo.record_edit(
            EditOp::Insert,
            EditSnapshot {
                text: "initX".to_string(),
                caret: 5,
                anchor: None,
            },
        );
        state.seed_undo_baseline();
        let restored = state.undo.undo();
        assert_eq!(
            restored.map(|s| s.text),
            Some("init".to_string()),
            "second seed must not reset a stack with recorded edits"
        );
    }

    #[test]
    fn preedit_forces_downstream_affinity() {
        use slate_text::Affinity;
        // Outside composition the stored seam affinity is authoritative.
        assert_eq!(
            caret_affinity_for_display(false, Affinity::Upstream),
            Affinity::Upstream
        );
        assert_eq!(
            caret_affinity_for_display(false, Affinity::Downstream),
            Affinity::Downstream
        );
        // During composition the caret binds Downstream regardless of the stored
        // value — pins the fix for the seam double-shift, so a future refactor
        // that threads cached affinity through preedit can't silently regress it.
        assert_eq!(
            caret_affinity_for_display(true, Affinity::Upstream),
            Affinity::Downstream
        );
        assert_eq!(
            caret_affinity_for_display(true, Affinity::Downstream),
            Affinity::Downstream
        );
    }

    #[test]
    fn caret_display_byte_tracks_preedit_cursor() {
        // Outside composition the caret sits at the display caret unchanged.
        assert_eq!(caret_display_byte(7, None), 7);

        // During composition the caret advances by the IME cursor offset, so it
        // moves through the composed run as you type instead of pinning at the
        // start.
        let p = Preedit {
            text: "かんじ".to_string(), // 9 bytes
            cursor_byte_offset: 6,
            selection: None,
        };
        assert_eq!(caret_display_byte(4, Some(&p)), 10);

        // A backend over-reporting the cursor offset is clamped to the end of
        // the composed text — never past it.
        let over = Preedit {
            text: "ab".to_string(), // 2 bytes
            cursor_byte_offset: 99,
            selection: None,
        };
        assert_eq!(caret_display_byte(4, Some(&over)), 6);

        // Zero offset binds to the composition start (display caret unchanged).
        let zero = Preedit {
            text: "xyz".to_string(),
            cursor_byte_offset: 0,
            selection: None,
        };
        assert_eq!(caret_display_byte(4, Some(&zero)), 4);
    }

    #[test]
    fn answer_marked_range_with_preedit() {
        let s = ImeState {
            text: "abc".to_string(),
            caret: 3,
            preedit: Some(Preedit {
                text: "xy".to_string(),
                cursor_byte_offset: 2,
                selection: None,
            }),
            ..Default::default()
        };
        assert_eq!(s.answer_marked_range(), Some(3..5));
    }
}
