//! Per-element IME state: committed buffer, caret, selection, composition,
//! and the paint/interaction cache that rides alongside it for framework-
//! internal cohesion.

use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use slate_platform::PhysicalRect;

use crate::elements::text_edit::undo::UndoStack;

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

#[cfg(test)]
mod tests {
    use super::*;

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
