//! UAX #9 bidirectional level resolution over a single line.
//!
//! Level resolution runs **once over the whole line** so context-sensitive
//! rules — neutral resolution (N0–N2) and number handling (EN/AN) — see the
//! surrounding strong characters. Each resolved level-run is then a single
//! direction that the native shaper receives in isolation, with no neutral or
//! number resolution left for the OS shaper to guess on a detached substring.
//!
//! Two consumers sit on this layer:
//! - the bidi segmenter emits one shaping span per **logical-order** run (then
//!   splits each on whitespace for the wrap fit's space-run model);
//! - line assembly reorders the runs present on a visual line into **visual**
//!   (left-to-right) order via [`visual_order`].

use std::ops::Range;

use unicode_bidi::{BidiInfo, Level};

use crate::types::Direction;

/// Reorder a sequence of run levels into visual (left-to-right) order.
///
/// `levels[i]` is the embedding level of the i-th run on a visual line, in
/// logical order. Returns indices into that sequence giving the display order
/// (UAX #9 rule L2: from the highest level down to the lowest odd level,
/// reverse each maximal contiguous span of runs at that level or above). Used
/// by line assembly: each wrapped line is a subset of the source line's runs,
/// so reordering happens per visual line rather than reusing whole-line order.
pub(crate) fn visual_order(levels: &[u8]) -> Vec<usize> {
    let n = levels.len();
    let mut order: Vec<usize> = (0..n).collect();
    let max_level = match levels.iter().copied().max() {
        Some(m) => m,
        None => return order,
    };
    // No odd (RTL) level means nothing reverses — logical order is visual.
    let Some(min_odd) = levels.iter().copied().filter(|l| l % 2 == 1).min() else {
        return order;
    };
    for lvl in (min_odd..=max_level).rev() {
        let mut i = 0;
        while i < n {
            if levels[i] >= lvl {
                let start = i;
                while i < n && levels[i] >= lvl {
                    i += 1;
                }
                order[start..i].reverse();
            } else {
                i += 1;
            }
        }
    }
    order
}

/// One maximal same-level run of a line, in logical (source) order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct BidiRun {
    /// Byte range in the source line.
    pub byte_range: Range<usize>,
    /// Resolved UAX #9 embedding level (even = LTR, odd = RTL).
    pub level: u8,
    /// Direction derived from `level` parity.
    pub direction: Direction,
}

/// Resolved bidi structure of one `\n`-free line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LineBidi {
    /// Paragraph base direction (auto-detected from the first strong character
    /// or forced by the `base` argument).
    pub base_direction: Direction,
    /// Level-runs in logical order. Empty for empty input.
    pub logical_runs: Vec<BidiRun>,
}

/// Map a [`Direction`] to a forced base embedding level.
fn to_level(d: Direction) -> Level {
    match d {
        Direction::Ltr => Level::ltr(),
        Direction::Rtl => Level::rtl(),
    }
}

/// Resolve `text` (one `\n`-free line) into logical-order level-runs.
///
/// `base = None` auto-detects the paragraph direction from the first strong
/// character (UAX #9 rule P2/P3); `Some(dir)` forces it. The returned runs
/// carry levels resolved paragraph-wide (rules through L1), so trailing
/// whitespace is reset to the base level.
pub(crate) fn resolve_line(text: &str, base: Option<Direction>) -> LineBidi {
    if text.is_empty() {
        return LineBidi {
            base_direction: base.unwrap_or_default(),
            logical_runs: Vec::new(),
        };
    }

    let info = BidiInfo::new(text, base.map(to_level));
    // A `\n`-free line is a single bidi paragraph; `new` always yields ≥1.
    let para = &info.paragraphs[0];
    let base_direction = if para.level.is_rtl() {
        Direction::Rtl
    } else {
        Direction::Ltr
    };

    // `visual_runs` applies L1 (trailing-whitespace / separator reset) and
    // returns the line's runs in visual order; sort back to logical order for
    // the wrap fit, which walks the source left to right.
    let (levels, runs) = info.visual_runs(para, para.range.clone());
    let mut logical_runs: Vec<BidiRun> = runs
        .into_iter()
        .map(|r| {
            let level = levels[r.start].number();
            BidiRun {
                byte_range: r,
                level,
                direction: if level.is_multiple_of(2) {
                    Direction::Ltr
                } else {
                    Direction::Rtl
                },
            }
        })
        .collect();
    logical_runs.sort_by_key(|r| r.byte_range.start);

    LineBidi {
        base_direction,
        logical_runs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the (level, direction) view of a resolution for terse asserts.
    fn run_levels(b: &LineBidi) -> Vec<(Range<usize>, u8, Direction)> {
        b.logical_runs
            .iter()
            .map(|r| (r.byte_range.clone(), r.level, r.direction))
            .collect()
    }

    #[test]
    fn pure_ltr_single_run_level_zero() {
        let b = resolve_line("abc", None);
        assert_eq!(b.base_direction, Direction::Ltr);
        assert_eq!(run_levels(&b), vec![(0..3, 0, Direction::Ltr)]);
        assert_eq!(visual_order(&[0]), vec![0]);
    }

    #[test]
    fn pure_rtl_hebrew_single_run_level_one() {
        // "אבג" — three Hebrew letters, 2 bytes each.
        let b = resolve_line("אבג", None);
        assert_eq!(b.base_direction, Direction::Rtl);
        assert_eq!(run_levels(&b), vec![(0..6, 1, Direction::Rtl)]);
    }

    #[test]
    fn ltr_base_with_trailing_rtl_run() {
        // "abc אבג": LTR base; the space is a neutral between L and R that
        // takes the embedding (LTR) level, merging into the leading run.
        let b = resolve_line("abc אבג", None);
        assert_eq!(b.base_direction, Direction::Ltr);
        assert_eq!(
            run_levels(&b),
            vec![(0..4, 0, Direction::Ltr), (4..10, 1, Direction::Rtl)]
        );
        // LTR base keeps the run order; the RTL run sits to the right.
        assert_eq!(visual_order(&[0, 1]), vec![0, 1]);
    }

    #[test]
    fn rtl_base_with_embedded_latin_reorders_visually() {
        // "אבג abc": RTL base (level 1). Embedded Latin lifts to level 2.
        let b = resolve_line("אבג abc", None);
        assert_eq!(b.base_direction, Direction::Rtl);
        assert_eq!(
            run_levels(&b),
            vec![(0..7, 1, Direction::Rtl), (7..10, 2, Direction::Ltr)]
        );
        // Visually the embedded Latin run is leftmost, Hebrew run rightmost.
        assert_eq!(visual_order(&[1, 2]), vec![1, 0]);
    }

    #[test]
    fn rtl_base_with_european_numbers_groups_ltr() {
        // "אבג 123": European numbers in an RTL context lift to an even level
        // so the digits read left-to-right as a group.
        let b = resolve_line("אבג 123", None);
        assert_eq!(b.base_direction, Direction::Rtl);
        let runs = run_levels(&b);
        let digit_run = runs.last().unwrap();
        assert_eq!(digit_run.0, 7..10);
        assert_eq!(digit_run.2, Direction::Ltr, "EN group reads LTR");
        assert_eq!(digit_run.1 % 2, 0, "EN run is even-level");
    }

    #[test]
    fn arabic_indic_digits_resolve_at_paragraph_level_not_isolated_guess() {
        // "abc ١٢٣ خ": LTR base. The Arabic-Indic digit run (AN) must resolve
        // to an EVEN (LTR-grouped) level from paragraph context — an isolated
        // substring shaper would guess RTL. The trailing Arabic letter (AL→R)
        // resolves to an ODD level.
        let b = resolve_line("abc ١٢٣ خ", None);
        assert_eq!(b.base_direction, Direction::Ltr);

        let digit_run = b
            .logical_runs
            .iter()
            .find(|r| r.byte_range.start == 4)
            .expect("digit run starts after 'abc '");
        assert_eq!(digit_run.level % 2, 0, "AN run resolves to even level");
        assert_eq!(digit_run.direction, Direction::Ltr);

        let letter_run = b.logical_runs.last().unwrap();
        assert_eq!(letter_run.level % 2, 1, "trailing Arabic letter is RTL");
        assert_eq!(letter_run.direction, Direction::Rtl);
    }

    #[test]
    fn isolate_run_does_not_panic_and_keeps_base() {
        // RLI … PDI around an Arabic letter inside Latin text.
        let b = resolve_line("a\u{2067}ب\u{2069}c", None);
        assert_eq!(b.base_direction, Direction::Ltr);
        assert!(!b.logical_runs.is_empty());
        // Byte coverage is gap-free and spans the whole line.
        assert_eq!(b.logical_runs.first().unwrap().byte_range.start, 0);
        assert_eq!(
            b.logical_runs.last().unwrap().byte_range.end,
            "a\u{2067}ب\u{2069}c".len()
        );
    }

    #[test]
    fn forced_base_overrides_first_strong() {
        // First strong is Latin (L) but force RTL base.
        let b = resolve_line("abc", Some(Direction::Rtl));
        assert_eq!(b.base_direction, Direction::Rtl);
    }

    #[test]
    fn empty_line_has_no_runs() {
        let b = resolve_line("", None);
        assert!(b.logical_runs.is_empty());
    }
}
