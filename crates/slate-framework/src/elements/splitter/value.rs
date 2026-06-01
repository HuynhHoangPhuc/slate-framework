//! Splitter ratio arithmetic: clamping the split ratio against per-pane minimum
//! sizes and mapping an absolute pointer position to a ratio. Pure math kept
//! separate from the element so the clamp/round-trip logic is unit-testable in
//! isolation (the off-by-one risks the phase plan calls out live here).

/// Logical-pixel keyboard step: how far an Arrow key nudges the divider.
pub(super) const STEP_PX: f32 = 16.0;

/// The geometry a [`Splitter`](super::Splitter) maps its split ratio over: the
/// main-axis length of the container, the divider thickness, and the minimum
/// size of each pane.
///
/// The ratio is a fraction of the *available* space — the container minus the
/// fixed divider — because that is exactly what Taffy distributes between the
/// two `flex_grow` panes. Pointer mapping therefore measures against `available`
/// and offsets by half the divider so the divider centre tracks the cursor.
#[derive(Clone, Copy, Debug)]
pub(super) struct SplitRange {
    /// Main-axis length of the splitter in logical px (width for a horizontal
    /// splitter, height for a vertical one).
    pub container: f32,
    /// Divider thickness in logical px (excluded from the panes' shared space).
    pub divider: f32,
    /// Minimum size of the first (left/top) pane in logical px.
    pub min_first: f32,
    /// Minimum size of the second (right/bottom) pane in logical px.
    pub min_second: f32,
}

impl SplitRange {
    /// Space the two panes actually share (container minus the fixed divider).
    fn available(&self) -> f32 {
        (self.container - self.divider).max(0.0)
    }

    /// The `[lo, hi]` ratio bounds implied by the per-pane minimums. `lo` is the
    /// smallest first-pane fraction that still satisfies `min_first`; `hi` the
    /// largest that still leaves `min_second` for the second pane.
    fn bounds(&self) -> (f32, f32) {
        let available = self.available();
        if available <= 0.0 {
            return (0.0, 1.0);
        }
        let lo = (self.min_first / available).clamp(0.0, 1.0);
        let hi = (1.0 - self.min_second / available).clamp(0.0, 1.0);
        (lo, hi)
    }

    /// Clamp `ratio` into the range allowed by the per-pane minimums. When the
    /// minimums can't both be satisfied (container too small), settle on the
    /// midpoint so neither pane is starved more than the other.
    pub fn clamp_ratio(&self, ratio: f32) -> f32 {
        let (lo, hi) = self.bounds();
        if lo > hi {
            return ((lo + hi) * 0.5).clamp(0.0, 1.0);
        }
        ratio.clamp(lo, hi)
    }

    /// Map an absolute pointer position (main-axis, window coords) to a clamped
    /// ratio, given the splitter's main-axis origin. Offsets by half the divider
    /// so the cursor lands on the divider *centre*, not its leading edge.
    pub fn ratio_at_pointer(&self, pointer_main: f32, origin_main: f32) -> f32 {
        let available = self.available();
        if available <= 0.0 {
            return self.clamp_ratio(0.5);
        }
        let offset = pointer_main - origin_main - self.divider * 0.5;
        self.clamp_ratio(offset / available)
    }

    /// Ratio delta for one keyboard step ([`STEP_PX`] of travel), or a small
    /// fixed fraction when the available space is unknown.
    pub fn step_ratio(&self) -> f32 {
        let available = self.available();
        if available > 0.0 {
            STEP_PX / available
        } else {
            0.05
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Divider 0 keeps `available == container == 400` so the arithmetic below
    // reads cleanly; a dedicated test covers the divider offset.
    const R: SplitRange = SplitRange {
        container: 400.0,
        divider: 0.0,
        min_first: 80.0,
        min_second: 120.0,
    };

    #[test]
    fn clamps_to_min_first() {
        // lo = 80/400 = 0.2; anything below clamps up.
        assert_eq!(R.clamp_ratio(0.0), 0.2);
        assert_eq!(R.clamp_ratio(0.1), 0.2);
    }

    #[test]
    fn clamps_to_min_second() {
        // hi = 1 - 120/400 = 0.7; anything above clamps down.
        assert_eq!(R.clamp_ratio(1.0), 0.7);
        assert_eq!(R.clamp_ratio(0.9), 0.7);
    }

    #[test]
    fn leaves_interior_untouched() {
        assert_eq!(R.clamp_ratio(0.5), 0.5);
    }

    #[test]
    fn impossible_minimums_settle_on_midpoint() {
        // 300 + 300 > 400 → lo (0.75) > hi (0.25) → midpoint 0.5.
        let tight = SplitRange {
            container: 400.0,
            divider: 0.0,
            min_first: 300.0,
            min_second: 300.0,
        };
        assert_eq!(tight.clamp_ratio(0.1), 0.5);
        assert_eq!(tight.clamp_ratio(0.9), 0.5);
    }

    #[test]
    fn pointer_maps_to_ratio_and_clamps() {
        // origin 0, container 400; pointer at 200 → 0.5.
        assert_eq!(R.ratio_at_pointer(200.0, 0.0), 0.5);
        // Account for a non-zero origin.
        assert_eq!(R.ratio_at_pointer(300.0, 100.0), 0.5);
        // Past the min-second edge clamps to hi.
        assert_eq!(R.ratio_at_pointer(390.0, 0.0), 0.7);
    }

    #[test]
    fn ratio_round_trips_through_px() {
        // First-pane px = clamped ratio × container.
        assert_eq!(R.clamp_ratio(0.5) * R.container, 200.0);
        // The ratio is clamped to min_first before mapping to px.
        assert_eq!(R.clamp_ratio(0.0) * R.container, 80.0);
    }

    #[test]
    fn zero_container_is_safe() {
        let z = SplitRange {
            container: 0.0,
            divider: 6.0,
            min_first: 80.0,
            min_second: 80.0,
        };
        assert_eq!(z.ratio_at_pointer(123.0, 0.0), 0.5);
        assert!((z.step_ratio() - 0.05).abs() < 1e-6);
    }

    #[test]
    fn step_ratio_scales_with_available_space() {
        assert!((R.step_ratio() - STEP_PX / 400.0).abs() < 1e-6);
    }

    #[test]
    fn divider_offset_centres_on_cursor() {
        // container 410, divider 10 → available 400. A cursor at the divider
        // centre (210 from origin) maps to the midpoint: (210 - 5) / 400... no
        // — pane1 is ratio×400, divider centre = ratio×400 + 5. So a cursor at
        // 205 (= 0.5×400 + 5) must yield 0.5.
        let d = SplitRange {
            container: 410.0,
            divider: 10.0,
            min_first: 0.0,
            min_second: 0.0,
        };
        assert!((d.ratio_at_pointer(205.0, 0.0) - 0.5).abs() < 1e-6);
        // The leading edge (origin) still clamps to 0.
        assert_eq!(d.ratio_at_pointer(0.0, 0.0), 0.0);
    }
}
