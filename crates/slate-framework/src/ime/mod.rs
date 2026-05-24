//! IME state registry + per-element composition state.
//!
//! The registry mirrors the [`FocusRegistry`](crate::focus::FocusRegistry)
//! lifecycle: cleared at the start of every prepaint, re-registered per
//! ime-capable element during the prepaint walk, then `prune_missing`
//! collapses any state belonging to unmounted elements.
//!
//! # Cache-then-query
//!
//! The platform `WindowImeDelegate` query channel does NOT traverse the
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

mod cache;
mod registry;
mod state;

pub use cache::{CachedImeQuery, PendingImeOp};
pub use registry::ImeRegistry;
pub use state::{BlinkState, ImeState, Preedit};

// ---------------------------------------------------------------------------
// Display caret affinity + byte helpers (shared by TextField + TextArea)
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
///
/// [`Affinity::Downstream`]: slate_text::Affinity::Downstream
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
}
