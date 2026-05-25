//! Byte ↔ pixel-x geometry helpers for caret math on shaped lines.
//!
//! Replaces the `.chars().count() * width` MVP that breaks on CJK ligatures,
//! Indic clusters, and emoji ZWJ sequences. The two entry points
//! ([`pixel_x_at_byte`] and [`byte_at_pixel_x`]) are byte-keyed end to end and
//! snap to grapheme boundaries so callers never have to.
//!
//! **Direction model:** a [`ShapedLine`](crate::ShapedLine) with an empty
//! `runs` list is a single implicit LTR run (pure-LTR / CJK) — cluster values
//! are non-decreasing in visual (= logical) order, and the original linear pen
//! walk handles it byte-identically. When `runs` is populated (mixed / RTL
//! text), the helpers switch to the run-aware path: visual x is laid out
//! left-to-right over the level-runs in visual order, and within each run
//! advances are summed forward (LTR) or in reverse (RTL) so the caret tracks
//! the correct visual edge of a logical byte. A boundary byte
//! (`runA.end == runB.start`) maps to two visual x-positions;
//! [`Affinity`](crate::types::Affinity) selects which — the run that *starts*
//! there (downstream, the default) or the run that *ends* there (upstream).
//!
//! - [`shape_utils`]: grapheme-snap helpers.
//! - [`bidi_runs`]: run-aware geometry primitives shared by caret + hit-test.
//! - [`caret`]: logical byte → visual caret x, caret motion, line edges,
//!   selection rects.
//! - [`hit_test`]: visual x → logical byte.

mod bidi_runs;
mod caret;
mod hit_test;
mod shape_utils;

pub use caret::{
    pixel_x_at_byte, run_caret_x, run_caret_x_at, run_caret_x_at_affinity, run_selection_rects,
    visual_caret_step, visual_line_edge,
};
pub use hit_test::{byte_at_pixel_x, run_byte_at_x};

/// Shared test factories (`glyph`, `line`, `dglyph`, `run_line`, `run`) used by
/// every submodule's `#[cfg(test)] mod tests`. Centralized to avoid duplication
/// without changing any test body.
#[cfg(test)]
pub(super) mod test_support {
    use crate::ShapedLine;
    use crate::types::{Direction, FontId, RunSpan, ShapedGlyph};

    pub(crate) fn glyph(cluster: u32, adv: f32) -> ShapedGlyph {
        ShapedGlyph {
            glyph_id: 1,
            font_id: FontId::PRIMARY,
            font_handle: crate::FontHandle::default(),
            x_advance_lpx: adv,
            position_lpx: [0.0, 0.0],
            cluster,
            direction: crate::types::Direction::Ltr,
        }
    }

    pub(crate) fn line(glyphs: Vec<ShapedGlyph>) -> ShapedLine {
        let width: f32 = glyphs.iter().map(|g| g.x_advance_lpx).sum();
        ShapedLine {
            glyphs,
            width_lpx: width,
            ascent_lpx: 10.0,
            descent_lpx: -2.0,
            y_offset_lpx: 0.0,
            base_direction: crate::types::Direction::Ltr,
            runs: Vec::new(),
        }
    }

    /// Glyph with an explicit run direction (RTL runs walk advances in reverse).
    pub(crate) fn dglyph(cluster: u32, adv: f32, dir: Direction) -> ShapedGlyph {
        ShapedGlyph {
            direction: dir,
            ..glyph(cluster, adv)
        }
    }

    /// Build a run-bearing line from glyphs (visual order) and runs (visual
    /// order). `width_lpx` is the glyph advance sum.
    pub(crate) fn run_line(glyphs: Vec<ShapedGlyph>, runs: Vec<RunSpan>) -> ShapedLine {
        let width: f32 = glyphs.iter().map(|g| g.x_advance_lpx).sum();
        ShapedLine {
            glyphs,
            width_lpx: width,
            ascent_lpx: 10.0,
            descent_lpx: -2.0,
            y_offset_lpx: 0.0,
            base_direction: Direction::Rtl,
            runs,
        }
    }

    pub(crate) fn run(range: std::ops::Range<usize>, dir: Direction) -> RunSpan {
        RunSpan {
            level: if dir == Direction::Rtl { 1 } else { 0 },
            byte_range: range,
            direction: dir,
        }
    }
}
