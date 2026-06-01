//! Shared 1-D roving-focus index math for keyboard-navigable lists.
//!
//! [`MenuList`](crate::MenuList), [`List`](crate::List),
//! [`VirtualList`](crate::VirtualList), and [`Tree`](crate::Tree) all move a
//! single active index up/down a column of rows, clamping at the ends (no wrap)
//! and skipping disabled rows — the one-dimensional reduction of the S0 grid
//! spike's roving pattern. The pure index helpers live here so each widget
//! doesn't re-derive them.

/// First enabled index scanning from `start` in `dir` (+1 / -1), clamped at the
/// ends (no wrap). Returns `start` if no enabled row exists in that direction.
pub(crate) fn step_enabled(disabled: &[bool], start: usize, dir: isize) -> usize {
    let n = disabled.len() as isize;
    let mut i = start as isize;
    loop {
        let next = i + dir;
        if next < 0 || next >= n {
            // Fell off the end with no enabled row in this direction: hold at the
            // original position rather than landing on a disabled/edge row.
            return start;
        }
        i = next;
        if !disabled[i as usize] {
            return i as usize;
        }
    }
}

/// First / last enabled index (for Home / End). Falls back to `0`.
pub(crate) fn end_enabled(disabled: &[bool], from_start: bool) -> usize {
    let n = disabled.len();
    if from_start {
        (0..n).find(|&i| !disabled[i]).unwrap_or(0)
    } else {
        (0..n).rev().find(|&i| !disabled[i]).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::{end_enabled, step_enabled};

    #[test]
    fn step_clamps_at_ends_when_all_enabled() {
        let d = [false, false, false];
        assert_eq!(step_enabled(&d, 0, -1), 0, "up at top stays");
        assert_eq!(step_enabled(&d, 2, 1), 2, "down at bottom stays");
        assert_eq!(step_enabled(&d, 1, 1), 2);
        assert_eq!(step_enabled(&d, 1, -1), 0);
    }

    #[test]
    fn step_skips_disabled_rows() {
        // index 1 disabled: from 0 down jumps to 2; from 2 up jumps to 0.
        let d = [false, true, false];
        assert_eq!(step_enabled(&d, 0, 1), 2);
        assert_eq!(step_enabled(&d, 2, -1), 0);
    }

    #[test]
    fn step_holds_when_no_enabled_neighbour() {
        // Only index 0 enabled: moving down finds nothing → stays at 0.
        let d = [false, true, true];
        assert_eq!(step_enabled(&d, 0, 1), 0);
    }

    #[test]
    fn home_end_pick_first_last_enabled() {
        let d = [true, false, false, true];
        assert_eq!(end_enabled(&d, true), 1, "Home skips leading disabled");
        assert_eq!(end_enabled(&d, false), 2, "End skips trailing disabled");
    }

    #[test]
    fn end_enabled_falls_back_when_all_disabled() {
        let d = [true, true];
        assert_eq!(end_enabled(&d, true), 0);
        assert_eq!(end_enabled(&d, false), 0);
    }
}
