//! Slider value arithmetic: clamping, step snapping, and the pixel ↔ value
//! mapping. Pure math kept separate from the element so it is unit-testable in
//! isolation (the off-by-one risks the phase plan calls out live here).

/// The numeric range + step a [`Slider`](super::Slider) maps over.
#[derive(Clone, Copy, Debug)]
pub(super) struct SliderRange {
    pub min: f32,
    pub max: f32,
    /// Snap granularity. `<= 0.0` means continuous (no snapping).
    pub step: f32,
}

impl SliderRange {
    /// Clamp `v` to `[min, max]` and snap to the nearest step from `min`.
    /// A non-positive step disables snapping (clamp only).
    pub fn clamp_step(&self, v: f32) -> f32 {
        let v = v.clamp(self.min, self.max);
        if self.step <= 0.0 {
            return v;
        }
        let steps = ((v - self.min) / self.step).round();
        (self.min + steps * self.step).clamp(self.min, self.max)
    }

    /// Position of `v` within the range as a `0.0..=1.0` fraction.
    pub fn fraction(&self, v: f32) -> f32 {
        if self.max <= self.min {
            return 0.0;
        }
        ((v - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }

    /// Map a `0.0..=1.0` fraction back to a (snapped) value.
    pub fn value_at_fraction(&self, f: f32) -> f32 {
        self.clamp_step(self.min + f.clamp(0.0, 1.0) * (self.max - self.min))
    }

    /// Map an absolute pointer X to a value, given the track's left edge and
    /// usable width (both in logical px). Out-of-track pointers clamp to the ends.
    pub fn value_at_pointer(&self, pointer_x: f32, track_x: f32, track_w: f32) -> f32 {
        if track_w <= 0.0 {
            return self.clamp_step(self.min);
        }
        let f = ((pointer_x - track_x) / track_w).clamp(0.0, 1.0);
        self.value_at_fraction(f)
    }

    /// Per-keystroke arrow increment: the step, or a 1/100 fallback when the
    /// slider is continuous so arrows still move it.
    pub fn step_amount(&self) -> f32 {
        if self.step > 0.0 {
            self.step
        } else {
            (self.max - self.min) / 100.0
        }
    }

    /// Larger PageUp/PageDown increment (10 steps, or 1/10 of the range).
    pub fn page_amount(&self) -> f32 {
        if self.step > 0.0 {
            self.step * 10.0
        } else {
            (self.max - self.min) / 10.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: SliderRange = SliderRange {
        min: 0.0,
        max: 100.0,
        step: 5.0,
    };

    #[test]
    fn clamps_below_and_above() {
        assert_eq!(R.clamp_step(-20.0), 0.0);
        assert_eq!(R.clamp_step(140.0), 100.0);
    }

    #[test]
    fn snaps_to_nearest_step() {
        assert_eq!(R.clamp_step(12.0), 10.0);
        assert_eq!(R.clamp_step(13.0), 15.0);
        assert_eq!(R.clamp_step(2.4), 0.0);
        assert_eq!(R.clamp_step(2.6), 5.0);
    }

    #[test]
    fn continuous_step_does_not_snap() {
        let c = SliderRange {
            min: 0.0,
            max: 1.0,
            step: 0.0,
        };
        assert!((c.clamp_step(0.337) - 0.337).abs() < 1e-6);
    }

    #[test]
    fn fraction_round_trips_through_value() {
        assert!((R.fraction(0.0) - 0.0).abs() < 1e-6);
        assert!((R.fraction(50.0) - 0.5).abs() < 1e-6);
        assert!((R.fraction(100.0) - 1.0).abs() < 1e-6);
        assert_eq!(R.value_at_fraction(0.5), 50.0);
    }

    #[test]
    fn pointer_maps_and_snaps() {
        // track [10, 210], width 200; pointer at 60 → fraction 0.25 → 25.
        assert_eq!(R.value_at_pointer(60.0, 10.0, 200.0), 25.0);
        // Left of the track clamps to min, right of it to max.
        assert_eq!(R.value_at_pointer(0.0, 10.0, 200.0), 0.0);
        assert_eq!(R.value_at_pointer(500.0, 10.0, 200.0), 100.0);
    }

    #[test]
    fn zero_width_track_holds_min() {
        assert_eq!(R.value_at_pointer(123.0, 10.0, 0.0), 0.0);
    }

    #[test]
    fn step_and_page_amounts() {
        assert_eq!(R.step_amount(), 5.0);
        assert_eq!(R.page_amount(), 50.0);
    }
}
