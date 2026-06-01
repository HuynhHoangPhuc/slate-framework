//! Thin-quad polyline: the line-pipeline substitute.
//!
//! The renderer has no line/stroke pipeline (`ScenePrimitive = {Shadow, Rect,
//! Glyph, Image}`), so a polyline is drawn as a run of small axis-aligned
//! squares sampled along each segment — the same "fake a stroke out of rects"
//! trick [`focus_ring`](crate::focus_ring) uses for its 4-rect ring. Fidelity is
//! intentionally focus-ring-grade (no anti-aliasing, no rotated quads); a real
//! stroke pipeline is out of scope until a bench/visual case demands it.

use slate_renderer::Lpx;
use slate_renderer::scene::{RectInstance, Scene};

use crate::types::{Bounds, Point};

/// Sample a polyline into stroke-width squares (one run per segment).
///
/// Each segment from `a` to `b` is sampled every half-stroke so any orientation
/// stays visually connected; the squares are `stroke_width` on a side, centred
/// on the sample point. A single point yields one square. Returned in draw
/// order — the first square is centred on `points[0]`, the last on the final
/// point. Pure (no scene access) so segment counts / endpoints are unit-testable.
pub fn polyline_quads(points: &[Point], stroke_width: f32) -> Vec<Bounds> {
    let s = stroke_width.max(1.0);
    let half = s * 0.5;
    let square = |p: Point| Bounds::from_origin_size(p.x - half, p.y - half, s, s);

    if points.is_empty() {
        return Vec::new();
    }
    if points.len() == 1 {
        return vec![square(points[0])];
    }

    let mut quads = Vec::new();
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let len = (dx * dx + dy * dy).sqrt();
        let steps = (len / half).ceil().max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            quads.push(square(Point::new(a.x + dx * t, a.y + dy * t)));
        }
    }
    quads
}

/// Emit a polyline into the scene as thin-quad squares of `color` (premultiplied
/// linear RGBA).
pub(crate) fn emit_polyline(scene: &mut Scene, points: &[Point], stroke_width: f32, color: [f32; 4]) {
    for q in polyline_quads(points, stroke_width) {
        scene.push_rect(RectInstance {
            rect: [Lpx(q.origin.x), Lpx(q.origin.y), Lpx(q.size.width), Lpx(q.size.height)],
            color,
            corner_radius: Lpx(0.0),
            _pad: [0.0; 3],
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f32, y: f32) -> Point {
        Point::new(x, y)
    }

    #[test]
    fn empty_input_yields_no_quads() {
        assert!(polyline_quads(&[], 2.0).is_empty());
    }

    #[test]
    fn single_point_yields_one_centred_square() {
        let q = polyline_quads(&[p(10.0, 20.0)], 4.0);
        assert_eq!(q.len(), 1);
        // Centre = origin + half-stroke.
        assert_eq!(q[0].origin.x + 2.0, 10.0);
        assert_eq!(q[0].origin.y + 2.0, 20.0);
    }

    #[test]
    fn endpoints_are_covered() {
        // Horizontal segment of length 10, stroke 2 → first square on (0,0),
        // last on (10,0).
        let q = polyline_quads(&[p(0.0, 0.0), p(10.0, 0.0)], 2.0);
        let first = q.first().unwrap();
        let last = q.last().unwrap();
        assert_eq!(first.origin.x + 1.0, 0.0);
        assert_eq!(last.origin.x + 1.0, 10.0);
    }

    #[test]
    fn diagonal_segment_is_sampled_into_a_connected_run() {
        // length ~14.1, half-stroke 1 → ~15 samples, all square-sized.
        let q = polyline_quads(&[p(0.0, 0.0), p(10.0, 10.0)], 2.0);
        assert!(q.len() > 10);
        assert!(q.iter().all(|b| b.size.width == 2.0 && b.size.height == 2.0));
    }
}
