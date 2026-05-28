//! Bidi/UAX-14 segmentation and per-word native shaping.
//!
//! Splits a line of text into shaping spans ([`Segment`]) via a [`LineSegmenter`]
//! ([`BidiSegmenter`] in production), then shapes each span once through the
//! backend to produce [`ShapedWord`] items the wrap layer can fit by pure
//! arithmetic.

use crate::backend::TextBackend;
use crate::bidi::{self, BidiRun};
use crate::error::TextError;
use crate::types::{Direction, ShapedGlyph};

/// A single whitespace-delimited word shaped in isolation.
///
/// `glyphs` carry word-origin-relative positions (`position_lpx[0]` starts at
/// 0); [`wrap_shaped_words`] shifts them by the running line pen when placing
/// the word. Produced by [`shape_words`], cached, and fit to any width with no
/// further shaping.
///
/// [`shape_words`]: super::shape_words
/// [`wrap_shaped_words`]: super::wrap_shaped_words
#[derive(Clone, Debug)]
pub struct ShapedWord {
    /// Glyphs with positions relative to the word origin (pen starts at 0).
    pub glyphs: Vec<ShapedGlyph>,
    /// Total advance width of the word in logical pixels.
    pub advance_width_lpx: f32,
    /// Ascent of the word (drives line ascent when first on a line).
    pub ascent_lpx: f32,
    /// Descent of the word (drives line descent when first on a line).
    pub descent_lpx: f32,
    /// UTF-8 byte span of the word in the original `text` passed to
    /// [`shape_words`]. Lets the multi-line wrap recover per-visual-line byte
    /// ranges (which `wrap_shaped_words` alone cannot, since it works on the
    /// pre-shaped glyph runs with no source pointer).
    ///
    /// [`shape_words`]: super::shape_words
    pub source_byte_range: std::ops::Range<usize>,
    /// `true` when this item is a run of ASCII spaces (U+0020) rather than a
    /// text word. A space run carries one glyph per space byte (each advancing
    /// `space_width`) so every space is independently caret-addressable. The
    /// wrap fit treats it as a soft-break candidate whose trailing copy is
    /// absorbed at a soft wrap but kept (visible) at a hard line end.
    pub is_space_run: bool,
    /// UAX #9 embedding level of the level-run this item belongs to (even = LTR,
    /// odd = RTL). `0` on the pure-LTR / CJK path. Line assembly reorders a
    /// line's items into visual order from these levels (rule L2).
    pub level: u8,
    /// `true` when this item is a single horizontal tab (U+0009). A tab has no
    /// fixed advance: line assembly advances the pen to the next tab stop
    /// (`ceil((pen+ε)/tab_width)·tab_width`), so `advance_width_lpx` is ignored
    /// for tabs. Each tab is its own caret-addressable item.
    pub is_tab: bool,
    /// `true` when a UAX #14 soft line break may fall immediately before this
    /// item. The wrap fit may only start a new line at an item with this set
    /// (plus the always-allowed first item). ASCII words after a space, CJK
    /// ideographs, and the piece after a hyphen all carry it; the first piece of
    /// a run with no break opportunity before it does not.
    pub break_before: bool,
}

/// Shape every whitespace-delimited word in `text` exactly once.
///
/// Returns the ordered items (text words interleaved with ASCII-space runs)
/// plus the shared single-space advance (shaped once, for callers that still
/// want it). Pair with [`wrap_shaped_words`] to fit the items to any width with
/// zero further shaping calls — so re-wrap on a resize is pure arithmetic.
///
/// Every ASCII space (U+0020) is preserved as its own glyph (see
/// `shape_words_in`); empty input yields an empty list, but a whitespace-only
/// string yields a single space-run item.
///
/// [`wrap_shaped_words`]: super::wrap_shaped_words
pub fn shape_words<B: TextBackend>(
    backend: &B,
    font: &B::Font,
    text: &str,
) -> Result<(Vec<ShapedWord>, f32), TextError> {
    // Shape a space once to get the inter-word advance (reused at every join).
    let space_width = backend
        .shape_line(font, " ")
        .map(|s| s.width_lpx)
        .unwrap_or(0.0);

    let words = shape_words_in(backend, font, text, 0)?;
    Ok((words, space_width))
}

/// A segmentation unit handed to the native shaper: a contiguous span of the
/// source text that is a single resolved direction (and, in later phases, a
/// single break-bounded level-run).
///
/// Today the only segmenter is [`WhitespaceSegmenter`], which produces maximal
/// ASCII-space / non-space runs, all `Ltr` at level 0 — reproducing the
/// historical `shape_words_in` byte scan exactly. The bidi segmenter replaces
/// it with real level-runs once direction resolution is wired in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Segment {
    /// Byte range within the text passed to [`LineSegmenter::segments`].
    pub byte_range: std::ops::Range<usize>,
    /// Resolved direction of this span.
    pub direction: Direction,
    /// UAX #9 embedding level (0 for the whitespace segmenter).
    pub level: u8,
    /// `true` when this span is a single horizontal tab (U+0009).
    pub is_tab: bool,
    /// `true` when a UAX #14 break opportunity falls before this span's start.
    pub break_before: bool,
}

/// Splits a line of text into shaping segments.
///
/// The seam that lets later phases swap whitespace-word segmentation for bidi
/// level-run segmentation without touching the shaping/fit plumbing. The unit
/// of segmentation is whatever the native shaper should receive in one call.
pub(crate) trait LineSegmenter {
    /// Segment `text` (a single `\n`-free run) into ordered shaping spans.
    fn segments(&self, text: &str) -> Vec<Segment>;
}

/// Whitespace oracle segmenter: maximal runs of ASCII spaces (U+0020)
/// interleaved with non-space spans, all `Ltr` at level 0.
///
/// Byte-for-byte equivalent to the historical `shape_words_in` scan. The bidi
/// segmenter superseded it on the production path; it survives as the oracle
/// for the LTR-identity regression gate (the bidi segmenter must reproduce its
/// spans on pure-LTR / CJK text), hence test-only.
#[cfg(test)]
pub(crate) struct WhitespaceSegmenter;

#[cfg(test)]
impl LineSegmenter for WhitespaceSegmenter {
    fn segments(&self, text: &str) -> Vec<Segment> {
        let bytes = text.as_bytes();
        let mut segs = Vec::new();
        let mut i = 0usize;
        while i < bytes.len() {
            let start = i;
            let is_space = bytes[i] == b' ';
            // Extend the run while its space-ness matches.
            while i < bytes.len() && (bytes[i] == b' ') == is_space {
                i += 1;
            }
            segs.push(Segment {
                byte_range: start..i,
                direction: Direction::Ltr,
                level: 0,
                is_tab: false,
                break_before: false,
            });
        }
        segs
    }
}

/// Production segmenter: resolves UAX #9 bidi levels paragraph-wide and computes
/// UAX #14 break opportunities over the whole line, then emits one shaping span
/// per (level-run × break-opportunity × kind) cell. Kinds are text, ASCII-space
/// run, and single tab. Each span carries its run's direction and level, a
/// `break_before` flag (a soft break may start a line here), and `is_tab`. Line
/// assembly reorders the spans into visual order.
///
/// For pure-LTR text whose break opportunities coincide with spaces (all of
/// ASCII-Latin), this yields the same byte ranges as [`WhitespaceSegmenter`], so
/// the shaping output is unchanged. CJK splits at every ideograph (so it can
/// finally wrap), and a hyphen splits the word after it — both intended changes.
pub(crate) struct BidiSegmenter {
    /// Forced paragraph base direction; `None` auto-detects (first strong char).
    pub base: Option<Direction>,
}

impl LineSegmenter for BidiSegmenter {
    fn segments(&self, text: &str) -> Vec<Segment> {
        let resolved = bidi::resolve_line(text, self.base);
        // Break opportunities are a line-global partition orthogonal to bidi
        // level-runs; the shaping unit is their intersection. Computed once over
        // the whole line so offsets are comparable across runs.
        let breaks = crate::linebreak::break_offsets(text);
        let mut segs = Vec::new();
        for run in &resolved.logical_runs {
            split_run(text, run, &breaks, &mut segs);
        }
        segs
    }
}

/// Split one level-run into shaping spans, tagging each with the run's direction
/// and level. Boundaries fall at: kind changes (text / ASCII-space / tab), each
/// individual tab (own caret-addressable stop), and interior UAX #14 break
/// offsets inside a text span. Appends to `out` in logical (source) order.
fn split_run(text: &str, run: &BidiRun, breaks: &[usize], out: &mut Vec<Segment>) {
    let bytes = text.as_bytes();
    let end = run.byte_range.end;
    let mut i = run.byte_range.start;
    while i < end {
        let start = i;
        let is_tab = bytes[i] == b'\t';
        if is_tab {
            // Tab: its own single-byte span (each advances to its own stop).
            i += 1;
        } else if bytes[i] == b' ' {
            // Maximal ASCII-space run — the absorption / soft-break unit.
            while i < end && bytes[i] == b' ' {
                i += 1;
            }
        } else {
            // Text: extend over chars until the next space, tab, or interior
            // break offset (so CJK splits per ideograph, "foo-bar" after '-').
            loop {
                let ch = text[i..end].chars().next().expect("i < end is on a char");
                i += ch.len_utf8();
                if i >= end
                    || bytes[i] == b' '
                    || bytes[i] == b'\t'
                    || crate::linebreak::is_break_before(breaks, i)
                {
                    break;
                }
            }
        }
        out.push(Segment {
            byte_range: start..i,
            direction: run.direction,
            level: run.level,
            is_tab,
            break_before: crate::linebreak::is_break_before(breaks, start),
        });
    }
}

/// Shape `segment` into an ordered run of items, recording each item's byte
/// span as an absolute offset into the larger document (`segment_start` is the
/// byte offset of `segment` within that document; pass 0 when `segment` is the
/// whole text).
///
/// Drives off [`BidiSegmenter`] via the [`LineSegmenter`] seam: each emitted
/// span is shaped once via `shape_segment`, and its glyphs are tagged with the
/// span direction. A span whose first byte is U+0020 is a space run (one glyph
/// per space byte, caret-addressable, soft-break candidate); a single U+0009 is
/// a tab (caret-addressable, pen-relative advance at assembly); other spans are
/// text words. Each item carries the segment's `break_before` (whether a soft
/// line break may start here) and `is_tab` flags. Other Unicode whitespace
/// (NBSP, …) stays inside the surrounding text word and shapes as part of it.
///
/// Shared by [`shape_words`] (single segment, offset 0) and the multi-line
/// paragraph shaper, which calls it once per `\n`-delimited paragraph so item
/// ranges stay absolute across the document.
pub(crate) fn shape_words_in<B: TextBackend>(
    backend: &B,
    font: &B::Font,
    segment: &str,
    segment_start: usize,
) -> Result<Vec<ShapedWord>, TextError> {
    let segmenter = BidiSegmenter { base: None };
    let mut items = Vec::new();
    for seg in segmenter.segments(segment) {
        let slice = &segment[seg.byte_range.clone()];
        // Homogeneous spans (segmenter never mixes kinds), so the first byte
        // determines the whole run's space-ness. Tabs are a separate kind.
        let is_space = !seg.is_tab && slice.as_bytes().first() == Some(&b' ');
        let mut shaped = backend.shape_segment(font, slice, seg.direction)?;
        for g in &mut shaped.glyphs {
            g.direction = seg.direction;
        }
        items.push(ShapedWord {
            glyphs: shaped.glyphs,
            advance_width_lpx: shaped.width_lpx,
            ascent_lpx: shaped.ascent_lpx,
            descent_lpx: shaped.descent_lpx,
            source_byte_range: segment_start + seg.byte_range.start
                ..segment_start + seg.byte_range.end,
            is_space_run: is_space,
            level: seg.level,
            is_tab: seg.is_tab,
            break_before: seg.break_before,
        });
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Project a segment to the fields the LTR-identity gate cares about: byte
    /// range, direction, level, tab-ness. `break_before` is new UAX #14 info the
    /// whitespace oracle does not model, so it is excluded from the comparison.
    fn seg_shape(s: &Segment) -> (std::ops::Range<usize>, Direction, u8, bool) {
        (s.byte_range.clone(), s.direction, s.level, s.is_tab)
    }

    /// LTR-identity gate: on ASCII-Latin text (UAX #14 breaks coincide with
    /// spaces) the bidi segmenter must emit the same span *boundaries* as the
    /// whitespace segmenter, so the shaping output is unchanged. CJK is excluded
    /// here — it now splits per ideograph (its own test below).
    #[test]
    fn bidi_segmenter_matches_whitespace_segmenter_on_ascii() {
        let bidi = BidiSegmenter { base: None };
        let ws = WhitespaceSegmenter;
        for text in ["", "hello", "ab cd", "a  b", " ab ", "the quick brown fox"] {
            let b: Vec<_> = bidi.segments(text).iter().map(seg_shape).collect();
            let w: Vec<_> = ws.segments(text).iter().map(seg_shape).collect();
            assert_eq!(b, w, "segmenters diverged on {text:?}");
        }
    }

    /// CJK has no spaces: the bidi segmenter must now split it at every UAX #14
    /// break (between ideographs) so it can wrap. Each non-first piece carries
    /// `break_before`.
    #[test]
    fn bidi_segmenter_splits_cjk_at_break_opportunities() {
        let segs = BidiSegmenter { base: None }.segments("日本語");
        let ranges: Vec<_> = segs.iter().map(|s| s.byte_range.clone()).collect();
        assert_eq!(ranges, vec![0..3, 3..6, 6..9]);
        assert!(!segs[0].break_before, "first piece has no break before it");
        assert!(segs[1].break_before && segs[2].break_before);
    }

    /// A break opportunity after a hyphen splits the word; the second piece is a
    /// soft-break start.
    #[test]
    fn bidi_segmenter_splits_after_hyphen() {
        let segs = BidiSegmenter { base: None }.segments("foo-bar");
        let ranges: Vec<_> = segs.iter().map(|s| s.byte_range.clone()).collect();
        assert_eq!(ranges, vec![0..4, 4..7]);
        assert!(segs[1].break_before);
    }

    /// Each tab is its own segment, flagged `is_tab`, never merged with text or
    /// with an adjacent tab.
    #[test]
    fn tab_is_an_isolated_segment() {
        let segs = BidiSegmenter { base: None }.segments("a\t\tb");
        let shapes: Vec<_> = segs
            .iter()
            .map(|s| (s.byte_range.clone(), s.is_tab))
            .collect();
        assert_eq!(
            shapes,
            vec![(0..1, false), (1..2, true), (2..3, true), (3..4, false)]
        );
    }

    /// On RTL / mixed text the bidi segmenter must tag runs with the resolved
    /// direction (the whitespace segmenter cannot).
    #[test]
    fn bidi_segmenter_tags_rtl_runs() {
        let segs = BidiSegmenter { base: None }.segments("abc אבג");
        // "abc" + " " are LTR; "אבג" is RTL.
        assert!(segs.iter().any(|s| s.direction == Direction::Rtl));
        assert!(segs.iter().any(|s| s.direction == Direction::Ltr));
        // Byte coverage is gap-free and ordered.
        assert_eq!(segs.first().unwrap().byte_range.start, 0);
        assert_eq!(segs.last().unwrap().byte_range.end, "abc אבג".len());
    }
}
