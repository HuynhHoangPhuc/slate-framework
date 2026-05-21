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
//! opportunities the wrap fit may or may not take. This is exact because the
//! document splitter ([`split_paragraphs`]) hard-breaks paragraphs at every
//! UAX #14 *mandatory* separator before this module ever sees a line, so a line
//! handed to [`break_offsets`] carries no interior mandatory separator. (The
//! earlier limitation — non-`\n` mandatory separators demoted to soft here — is
//! **resolved** at the document layer by [`split_paragraphs`].)

use unicode_linebreak::linebreaks;

/// Split `text` into paragraphs at UAX #14 *mandatory* breaks.
///
/// Recognized terminators: LF (U+000A), CR (U+000D), CRLF (`\r\n`, one unit),
/// VT (U+000B), FF (U+000C), NEL (U+0085), LS (U+2028), PS (U+2029). Yields
/// `(absolute_start, content)` where `content` excludes the trailing
/// terminator. Terminator bytes fold into the preceding paragraph's coverage —
/// the caller derives coverage from the next paragraph's start — so byte ranges
/// stay gap-free. Semantics match `str::split` over the separator set: a
/// trailing terminator produces a trailing empty paragraph; separator-free text
/// produces a single paragraph.
pub(crate) fn split_paragraphs(text: &str) -> Vec<(usize, &str)> {
    let mut out: Vec<(usize, &str)> = Vec::new();
    let mut start = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let term_len = match c {
            '\n' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}' => c.len_utf8(),
            // CRLF is a single terminator; a lone CR breaks on its own.
            '\r' => {
                if matches!(chars.peek(), Some(&(_, '\n'))) {
                    chars.next();
                    2
                } else {
                    1
                }
            }
            _ => continue,
        };
        out.push((start, &text[start..i]));
        start = i + term_len;
    }
    // Final paragraph (empty when text ends on a terminator → split-parity).
    out.push((start, &text[start..]));
    out
}

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

    // ── split_paragraphs ──────────────────────────────────────────────────

    /// Reference splitter: the pre-change `text.split('\n')` + cumulative
    /// `+len+1` offset accounting. The new `split_paragraphs` must match it
    /// byte-for-byte on `\n`-only / separator-free text (regression contract).
    fn split_newline_reference(text: &str) -> Vec<(usize, &str)> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        for para in text.split('\n') {
            out.push((offset, para));
            offset += para.len() + 1;
        }
        out
    }

    #[test]
    fn split_paragraphs_matches_split_newline_on_lf_only() {
        for s in ["", "a", "a\nb", "a\n", "\n", "a\n\nb", "日本語\nx"] {
            assert_eq!(
                split_paragraphs(s),
                split_newline_reference(s),
                "split_paragraphs disagreed with split('\\n') reference for {s:?}"
            );
        }
    }

    #[test]
    fn crlf_is_one_terminator() {
        // CRLF = 2 bytes consumed → next paragraph starts at 3, not 2.
        assert_eq!(split_paragraphs("a\r\nb"), vec![(0, "a"), (3, "b")]);
    }

    #[test]
    fn lone_cr_breaks() {
        assert_eq!(split_paragraphs("a\rb"), vec![(0, "a"), (2, "b")]);
    }

    #[test]
    fn unicode_separators_break() {
        // VT / FF: 1-byte. NEL: 2-byte. LS / PS: 3-byte. Next-start offset
        // reflects each terminator's UTF-8 length.
        assert_eq!(split_paragraphs("a\u{000B}b"), vec![(0, "a"), (2, "b")]);
        assert_eq!(split_paragraphs("a\u{000C}b"), vec![(0, "a"), (2, "b")]);
        assert_eq!(split_paragraphs("a\u{0085}b"), vec![(0, "a"), (3, "b")]);
        assert_eq!(split_paragraphs("a\u{2028}b"), vec![(0, "a"), (4, "b")]);
        assert_eq!(split_paragraphs("a\u{2029}b"), vec![(0, "a"), (4, "b")]);
    }

    #[test]
    fn trailing_terminator_yields_empty_paragraph() {
        assert_eq!(split_paragraphs("a\r\n"), vec![(0, "a"), (3, "")]);
    }

    #[test]
    fn content_excludes_terminator() {
        // No returned content slice may contain any mandatory-separator byte.
        let seps = ['\n', '\r', '\u{000B}', '\u{000C}', '\u{0085}', '\u{2028}', '\u{2029}'];
        for (_, content) in split_paragraphs("a\r\nb\nc\u{2028}d\u{0085}\u{000C}e") {
            assert!(
                !content.chars().any(|c| seps.contains(&c)),
                "content {content:?} contained a separator byte"
            );
        }
    }

    #[test]
    fn mixed_terminators() {
        // CRLF, LF, then LS — four paragraphs, gap-free ascending starts.
        let spans = split_paragraphs("a\r\nb\nc\u{2028}d");
        assert_eq!(spans, vec![(0, "a"), (3, "b"), (5, "c"), (9, "d")]);
        for w in spans.windows(2) {
            assert!(w[0].0 < w[1].0, "starts must strictly ascend");
        }
    }
}
