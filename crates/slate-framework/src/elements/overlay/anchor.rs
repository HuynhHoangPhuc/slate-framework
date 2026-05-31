//! Pure anchor-positioning solver for overlays.
//!
//! Given a target rect to anchor to, the overlay content's size, a [`Placement`]
//! (preferred side + cross-axis alignment), and the window/viewport size, this
//! computes the content's top-left origin in window-relative logical pixels. It
//! **flips** to the opposite side when the preferred side would overflow the
//! viewport, and **shifts** along the cross axis to keep the content on-screen.
//!
//! The solver is pure geometry — no GPU, layout-tree, or context state — so
//! every branch is exhaustively unit-testable in isolation.

use crate::types::{Bounds, Point, Size};

/// Which side of the anchor rect the overlay is placed on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Side {
    /// Above the anchor (overlay bottom edge sits above the anchor top).
    Top,
    /// Below the anchor (overlay top edge sits below the anchor bottom).
    Bottom,
    /// Left of the anchor.
    Left,
    /// Right of the anchor.
    Right,
}

/// Cross-axis alignment of the overlay relative to the anchor.
///
/// The cross axis is horizontal for [`Side::Top`]/[`Side::Bottom`] and vertical
/// for [`Side::Left`]/[`Side::Right`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Align {
    /// Align the overlay's leading edge to the anchor's leading edge.
    Start,
    /// Center the overlay on the anchor along the cross axis.
    Center,
    /// Align the overlay's trailing edge to the anchor's trailing edge.
    End,
}

/// Full placement spec: a preferred [`Side`] plus cross-axis [`Align`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    /// Preferred side; the solver flips to the opposite side on overflow.
    pub side: Side,
    /// Cross-axis alignment relative to the anchor.
    pub align: Align,
}

impl Placement {
    /// Place below the anchor, centered (the common dropdown/tooltip default).
    pub const fn bottom() -> Self {
        Self {
            side: Side::Bottom,
            align: Align::Center,
        }
    }

    /// Place above the anchor, centered.
    pub const fn top() -> Self {
        Self {
            side: Side::Top,
            align: Align::Center,
        }
    }

    /// Place left of the anchor, centered.
    pub const fn left() -> Self {
        Self {
            side: Side::Left,
            align: Align::Center,
        }
    }

    /// Place right of the anchor, centered.
    pub const fn right() -> Self {
        Self {
            side: Side::Right,
            align: Align::Center,
        }
    }

    /// Override the cross-axis alignment, keeping the side.
    pub const fn with_align(mut self, align: Align) -> Self {
        self.align = align;
        self
    }
}

impl Default for Placement {
    fn default() -> Self {
        Self::bottom()
    }
}

/// Solve the overlay content's top-left origin (window-relative logical px).
///
/// `anchor` is the target rect (absolute coords), `content` the overlay's
/// measured size, `viewport` the window size, and `gap` the offset between the
/// anchor edge and the overlay along the main axis.
///
/// Main axis = the placement side's axis (vertical for top/bottom). The result
/// flips to the opposite side when the preferred side cannot fit the content
/// within `viewport`, then clamps so the overlay never leaves the viewport. The
/// cross axis is positioned per [`Align`] and shifted to stay on-screen.
pub fn solve(anchor: Bounds, content: Size, placement: Placement, viewport: Size, gap: f32) -> Point {
    match placement.side {
        Side::Top | Side::Bottom => {
            let y = place_main(
                anchor.origin.y,
                anchor.size.height,
                content.height,
                viewport.height,
                gap,
                placement.side == Side::Bottom,
            );
            let x = place_cross(
                anchor.origin.x,
                anchor.size.width,
                content.width,
                viewport.width,
                placement.align,
            );
            Point::new(x, y)
        }
        Side::Left | Side::Right => {
            let x = place_main(
                anchor.origin.x,
                anchor.size.width,
                content.width,
                viewport.width,
                gap,
                placement.side == Side::Right,
            );
            let y = place_cross(
                anchor.origin.y,
                anchor.size.height,
                content.height,
                viewport.height,
                placement.align,
            );
            Point::new(x, y)
        }
    }
}

/// Main-axis (1-D) placement with flip + on-screen clamp.
///
/// `a0`/`alen` are the anchor's start and length along this axis, `clen` the
/// content length, `vlen` the viewport length. `prefer_after` chooses the
/// after-anchor side (bottom/right) as the preference; the solver flips to the
/// opposite side when the preference can't fit but the other side can, picks
/// the larger-space side when neither fits, then clamps into `[0, vlen-clen]`.
fn place_main(a0: f32, alen: f32, clen: f32, vlen: f32, gap: f32, prefer_after: bool) -> f32 {
    let after = a0 + alen + gap;
    let before = a0 - gap - clen;
    let fits_after = after + clen <= vlen;
    let fits_before = before >= 0.0;
    let space_after = vlen - (a0 + alen);
    let space_before = a0;

    let pos = if prefer_after {
        if fits_after {
            after
        } else if fits_before {
            before
        } else if space_after >= space_before {
            after
        } else {
            before
        }
    } else if fits_before {
        before
    } else if fits_after {
        after
    } else if space_before >= space_after {
        before
    } else {
        after
    };

    pos.clamp(0.0, (vlen - clen).max(0.0))
}

/// Cross-axis (1-D) alignment with on-screen shift.
///
/// Positions the content per [`Align`] relative to the anchor, then clamps into
/// `[0, vlen-clen]` so it stays within the viewport (the "shift" behavior).
fn place_cross(a0: f32, alen: f32, clen: f32, vlen: f32, align: Align) -> f32 {
    let pos = match align {
        Align::Start => a0,
        Align::Center => a0 + alen / 2.0 - clen / 2.0,
        Align::End => a0 + alen - clen,
    };
    pos.clamp(0.0, (vlen - clen).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: Size = Size::new(1000.0, 800.0);

    fn anchor_at(x: f32, y: f32, w: f32, h: f32) -> Bounds {
        Bounds::from_origin_size(x, y, w, h)
    }

    #[test]
    fn bottom_centered_fits() {
        // Anchor mid-screen, plenty of room below.
        let a = anchor_at(400.0, 300.0, 200.0, 40.0);
        let p = solve(a, Size::new(100.0, 60.0), Placement::bottom(), VIEWPORT, 4.0);
        assert_eq!(p.y, 344.0); // 300 + 40 + 4
        assert_eq!(p.x, 450.0); // center: 400 + 100 - 50
    }

    #[test]
    fn flips_to_top_when_no_room_below() {
        // Anchor near the bottom edge: below doesn't fit, above does.
        let a = anchor_at(400.0, 760.0, 200.0, 30.0);
        let p = solve(a, Size::new(100.0, 60.0), Placement::bottom(), VIEWPORT, 4.0);
        assert_eq!(p.y, 696.0); // flipped above: 760 - 4 - 60
    }

    #[test]
    fn flips_to_bottom_when_no_room_above() {
        let a = anchor_at(400.0, 10.0, 200.0, 30.0);
        let p = solve(a, Size::new(100.0, 60.0), Placement::top(), VIEWPORT, 4.0);
        assert_eq!(p.y, 44.0); // flipped below: 10 + 30 + 4
    }

    #[test]
    fn shifts_right_at_left_edge() {
        // Centered placement would push x negative; shift clamps to 0.
        let a = anchor_at(10.0, 300.0, 20.0, 40.0);
        let p = solve(a, Size::new(200.0, 60.0), Placement::bottom(), VIEWPORT, 4.0);
        assert_eq!(p.x, 0.0);
    }

    #[test]
    fn shifts_left_at_right_edge() {
        let a = anchor_at(980.0, 300.0, 20.0, 40.0);
        let p = solve(a, Size::new(200.0, 60.0), Placement::bottom(), VIEWPORT, 4.0);
        assert_eq!(p.x, 800.0); // clamped to vlen - clen = 1000 - 200
    }

    #[test]
    fn right_side_centered_vertically() {
        let a = anchor_at(400.0, 300.0, 50.0, 100.0);
        let p = solve(a, Size::new(120.0, 40.0), Placement::right(), VIEWPORT, 6.0);
        assert_eq!(p.x, 456.0); // 400 + 50 + 6
        assert_eq!(p.y, 330.0); // center: 300 + 50 - 20
    }

    #[test]
    fn left_side_aligns_start() {
        let a = anchor_at(400.0, 300.0, 50.0, 100.0);
        let p = solve(
            a,
            Size::new(120.0, 40.0),
            Placement::left().with_align(Align::Start),
            VIEWPORT,
            6.0,
        );
        assert_eq!(p.x, 274.0); // 400 - 6 - 120
        assert_eq!(p.y, 300.0); // start: anchor top
    }

    #[test]
    fn neither_side_fits_picks_larger_space() {
        // Tall content; anchor low so there's more room above than below.
        let a = anchor_at(400.0, 600.0, 200.0, 40.0);
        let content = Size::new(100.0, 1000.0); // taller than viewport
        let p = solve(a, content, Placement::bottom(), VIEWPORT, 4.0);
        // space_below = 800-640 = 160; space_above = 600 → above wins, clamped to 0.
        assert_eq!(p.y, 0.0);
    }

    #[test]
    fn content_wider_than_viewport_clamps_to_zero() {
        let a = anchor_at(400.0, 300.0, 200.0, 40.0);
        let p = solve(a, Size::new(1200.0, 60.0), Placement::bottom(), VIEWPORT, 4.0);
        assert_eq!(p.x, 0.0);
    }

    #[test]
    fn default_placement_is_bottom_centered() {
        assert_eq!(Placement::default(), Placement::bottom());
    }
}
