//! Clipboard copy / cut / paste over [`ImeState`](crate::ime::ImeState).
//!
//! The `multiline` flag is the only difference between `TextField` and
//! `TextArea` paste handling: single-line strips every `\n`/`\r`; multi-line
//! normalizes CRLF→LF and keeps the newlines. Copy/cut just operate on the
//! current selection and are layout-agnostic.

use crate::ime::ImeState;

use super::grapheme::insert_text_at;
use super::ops::{delete_selection, record_edit, reset_blink, selection_range};
use super::undo::EditOp;

/// The currently-selected text, or `None` when the selection is empty.
/// Used for both copy and cut (the cut handler additionally mutates state via
/// [`apply_cut`]).
pub(crate) fn selected_text(state: &ImeState) -> Option<String> {
    selection_range(state).map(|(lo, hi)| state.text[lo..hi].to_string())
}

/// Normalize raw clipboard text for insertion.
///
/// - `multiline = false`: strip every `\n` and `\r` (single-line fields never
///   hold line breaks).
/// - `multiline = true`: convert CRLF and lone CR to LF, keep the newlines.
pub(crate) fn clean_paste(raw: &str, multiline: bool) -> String {
    if multiline {
        raw.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        raw.chars().filter(|c| *c != '\n' && *c != '\r').collect()
    }
}

/// Cut: delete the active selection, record a discrete undo step, reset the
/// blink. Returns the post-cut text. Caller has already copied the selection
/// to the platform clipboard.
pub(crate) fn apply_cut(state: &mut ImeState) -> String {
    delete_selection(state);
    record_edit(state, EditOp::Discrete);
    reset_blink(state);
    state.text.clone()
}

/// Paste already-[`clean_paste`]d text at the caret. Aborts any active
/// composition (policy R5), replaces the selection, inserts, records a
/// discrete undo step, and resets the blink. Returns the post-paste text.
pub(crate) fn apply_paste(state: &mut ImeState, cleaned: &str) -> String {
    state.preedit = None;
    delete_selection(state);
    let old_caret = state.caret;
    let new_caret = insert_text_at(&mut state.text, old_caret, cleaned);
    state.caret = new_caret;
    record_edit(state, EditOp::Discrete);
    reset_blink(state);
    state.text.clone()
}

// ---------------------------------------------------------------------------
// Unit tests — paste normalization
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_single_line_strips_newlines_keeps_other_chars() {
        // Mirrors the previous TextField `paste_newline_stripping_keeps_other_chars`.
        let cleaned = clean_paste("line1\nline2\r\nline3", false);
        assert_eq!(cleaned, "line1line2line3");
    }

    #[test]
    fn paste_multiline_keeps_lf_and_normalizes_crlf() {
        assert_eq!(clean_paste("a\r\nb", true), "a\nb");
        assert_eq!(clean_paste("a\rb", true), "a\nb");
        assert_eq!(clean_paste("a\nb", true), "a\nb");
    }
}
