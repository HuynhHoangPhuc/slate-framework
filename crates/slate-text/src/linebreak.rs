//! UAX #14 line-break opportunities over a single `\n`-free line.
//!
//! Wraps the `unicode-linebreak` crate to answer one question the wrap fit
//! needs: *between which byte offsets may a soft line break fall?* CJK has no
//! ASCII spaces, so a space-only split can never wrap it — UAX #14 supplies the
//! algorithmic break points (between ideographs, after hyphens, after spaces,
//! …) for every script. Dictionary-based segmentation (Thai, Khmer, Lao) is out
//! of scope; those scripts get only their algorithmic opportunities.
//!
//! The crate yields a break offset *before which* a break may occur, and always
//! emits a final `(text.len(), Mandatory)` end-of-text marker. That end marker
//! is the line terminus, not an interior opportunity, so [`break_offsets`] drops
//! it (and the `0` position, which is never an interior break either).
//!
//! **Mandatory vs. allowed.** [`break_offsets`] keeps both `Mandatory` and
//! `Allowed` interior offsets but treats them identically — as *soft*
//! opportunities the wrap fit may or may not take. This is exact for the only
//! mandatory separator that reaches a line, the bare `\n`: the document splitter
//! already cuts on `\n`, so a `\n`-free line carries no interior `\n` to force.
//! It is a *known limitation* for the other UAX #14 mandatory separators
//! (U+2028/2029 line/paragraph separator, FF, VT, NEL): they are currently
//! demoted to soft opportunities here rather than forcing a break. Forcing them
//! is deferred until the document splitter recognises them.

use unicode_linebreak::linebreaks;

/// Interior UAX #14 break opportunities in `text`, as byte offsets *before
/// which* a soft break may fall.
///
/// Excludes `0` and `text.len()` (the line's own ends are not interior break
/// points). Offsets are returned in ascending order and always land on UTF-8
/// char boundaries (the crate breaks between chars), so a caller may split the
/// line at any returned offset without splitting a code point. Combining marks
/// generally stay attached: UAX #14 rule LB9 glues most combining marks to the
/// preceding base, so no break is offered between them — but this is a
/// code-point-level guarantee, not a grapheme-cluster one, and does not cover
/// every cluster shape (e.g. marks after a space, or ZWJ emoji sequences).
pub(crate) fn break_offsets(text: &str) -> Vec<usize> {
    linebreaks(text)
        .filter_map(|(offset, opportunity)| {
            // Drop the end-of-text marker and any spurious 0; keep both
            // Allowed and Mandatory interior opportunities.
            let _ = opportunity;
            (offset > 0 && offset < text.len()).then_some(offset)
        })
        .collect()
}

/// `true` when a soft break may fall immediately before byte `offset` in a line
/// whose interior break offsets are `breaks` (from [`break_offsets`]).
#[inline]
pub(crate) fn is_break_before(breaks: &[usize], offset: usize) -> bool {
    breaks.binary_search(&offset).is_ok()
}

/// Whether the crate marks `offset` as a *mandatory* break (e.g. an interior
/// `\n`/line separator). The document splitter removes `\n`, so this is only
/// used to assert that a `\n`-free line carries no interior mandatory break.
#[cfg(test)]
fn mandatory_offsets(text: &str) -> Vec<usize> {
    use unicode_linebreak::BreakOpportunity;
    linebreaks(text)
        .filter_map(|(offset, opportunity)| {
            (offset < text.len() && opportunity == BreakOpportunity::Mandatory).then_some(offset)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_breaks_after_spaces_only() {
        // "a b c": breaks fall before 'b' and before 'c' (after each space) —
        // the same boundaries the ASCII space split already uses.
        assert_eq!(break_offsets("a b c"), vec![2, 4]);
    }

    #[test]
    fn single_space_one_break() {
        // "Hello world!": one break, before 'w'.
        assert_eq!(break_offsets("Hello world!"), vec![6]);
    }

    #[test]
    fn no_break_inside_a_word() {
        assert_eq!(break_offsets("abc"), Vec::<usize>::new());
    }

    #[test]
    fn empty_text_has_no_breaks() {
        assert_eq!(break_offsets(""), Vec::<usize>::new());
    }

    #[test]
    fn hyphen_offers_a_break_after_it() {
        // "foo-bar": UAX #14 allows a break after the hyphen (before 'b'),
        // which the space-only split never produced — a new break point.
        assert_eq!(break_offsets("foo-bar"), vec![4]);
    }

    #[test]
    fn cjk_breaks_between_every_ideograph() {
        // "日本語" — three 3-byte ideographs. UAX #14 allows a break between
        // each pair, so a space-less CJK run becomes wrappable.
        assert_eq!(break_offsets("日本語"), vec![3, 6]);
    }

    #[test]
    fn cjk_offsets_land_on_char_boundaries() {
        let text = "日本語のテキスト";
        for &b in &break_offsets(text) {
            assert!(text.is_char_boundary(b), "break {b} split a code point");
        }
    }

    #[test]
    fn newline_free_line_has_no_interior_mandatory_break() {
        // The document splitter strips `\n`; a `\n`-free line must carry no
        // interior mandatory break (only the end-of-text terminus, excluded).
        assert!(mandatory_offsets("a b c").is_empty());
        assert!(mandatory_offsets("日本語").is_empty());
    }

    #[test]
    fn explicit_newline_is_a_mandatory_interior_break() {
        // Sanity-check the oracle itself: a `\n` *is* a mandatory break before
        // the following char (offset 2 in "a\nb"). This is why the document
        // layer splits on `\n` before this module ever sees a line.
        assert_eq!(mandatory_offsets("a\nb"), vec![2]);
    }
}
