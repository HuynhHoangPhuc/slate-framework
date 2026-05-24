//! Read-side IME snapshot consumed by `WindowImeDelegate`, plus the deferred
//! mutation queue drained by `AppState` after dispatch unwinds.

use std::ops::Range;

use slate_platform::{PhysicalRect, WindowId};

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
