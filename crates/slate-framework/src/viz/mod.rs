//! Primitive-drawn data visualization: simple charts built from `Rect` and
//! thin-quad primitives only — **no chart library, no new renderer pipeline**.
//!
//! The renderer's `ScenePrimitive` set is `{Shadow, Rect, Glyph, Image}` (no
//! line/stroke pipeline), so lines are faked from batched thin rects via
//! [`polyline_quads`] — mirroring how [`focus_ring`](crate::focus_ring) fakes a
//! stroke from four rects. Scope is deliberately small: a [`BarChart`] and a
//! [`Sparkline`]. Axes, legends, multi-series, and anti-aliased strokes are out
//! of scope (revisit only on bench/visual evidence, per the profiling-first
//! culture). Charts are leaf, non-interactive elements exposing a single `Image`
//! accessibility node with a generated text summary.

mod bar;
mod line;
mod polyline;

use slate_renderer::Lpx;
use slate_renderer::scene::{RectInstance, Scene};

use crate::types::Bounds;

pub use bar::BarChart;
pub use line::Sparkline;
pub use polyline::polyline_quads;

/// Push one opaque, square-cornered filled rect to the scene (shared by the
/// charts; premultiplied linear RGBA).
pub(super) fn fill_rect(scene: &mut Scene, b: Bounds, color: [f32; 4]) {
    scene.push_rect(RectInstance {
        rect: [Lpx(b.origin.x), Lpx(b.origin.y), Lpx(b.size.width), Lpx(b.size.height)],
        color,
        corner_radius: Lpx(0.0),
        _pad: [0.0; 3],
    });
}

/// Build a chart's accessible summary, e.g. `"Sparkline (CPU): 24 points, min
/// 3.0, max 39.6"`. Charts are read whole, so this string is the entire a11y
/// content. An empty series reads as "no data".
pub(super) fn series_summary(kind: &str, label: Option<&str>, values: &[f32]) -> String {
    let head = match label {
        Some(l) => format!("{kind} ({l})"),
        None => kind.to_string(),
    };
    if values.is_empty() {
        return format!("{head}: no data");
    }
    let min = values.iter().copied().fold(f32::MAX, f32::min);
    let max = values.iter().copied().fold(f32::MIN, f32::max);
    format!("{head}: {} points, min {min:.1}, max {max:.1}", values.len())
}

#[cfg(test)]
mod tests {
    use super::series_summary;

    #[test]
    fn summary_includes_count_and_extremes() {
        let s = series_summary("Sparkline", Some("CPU"), &[3.0, 39.6, 12.0]);
        assert_eq!(s, "Sparkline (CPU): 3 points, min 3.0, max 39.6");
    }

    #[test]
    fn summary_without_label_omits_parens() {
        let s = series_summary("Bar chart", None, &[1.0, 2.0]);
        assert_eq!(s, "Bar chart: 2 points, min 1.0, max 2.0");
    }

    #[test]
    fn summary_empty_series_reads_no_data() {
        assert_eq!(series_summary("Bar chart", None, &[]), "Bar chart: no data");
    }
}
