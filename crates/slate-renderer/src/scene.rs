//! `Scene` — per-frame container of instanced primitive data, plus the four
//! GPU-bound `*Instance` structs and the `Layer` range descriptor.
//!
//! # Design summary (see `plans/.../phase-01-scene-primitive-types.md`)
//!
//! Pure CPU-side data structures: no GPU code lives here. The renderer
//! (Phase 7) will read `Scene` once per frame, slice each `Vec` into a
//! single `Queue::write_buffer` per pipeline, and walk `layers` in
//! push-order issuing instanced draws.
//!
//! ## Struct-of-Arrays layout
//!
//! ```text
//! Scene
//! ├── rects:    Vec<RectInstance>
//! ├── shadows:  Vec<ShadowInstance>
//! ├── images:   Vec<ImageInstance>
//! ├── glyphs:   Vec<GlyphInstance>
//! └── layers:   Vec<Layer>     // half-open ranges into the four vecs above
//! ```
//!
//! ## Painter's-algorithm ordering
//!
//! Within each layer the renderer always walks **shadows → rects → glyphs →
//! images** (matches Zed's `PrimitiveKind` tiebreak — images sit on top of
//! glyphs because UI patterns put avatars/badges over labels). Layers
//! themselves are drawn in push order. `Scene` only records ranges; the
//! draw order itself is encoded by the renderer.
//!
//! ## Color contract
//!
//! Every color/tint field is **linear-space, premultiplied** RGBA. Use
//! [`crate::srgb_u8_to_linear_premul`] at the boundary. The pipeline blend
//! state is `One/OneMinusSrcAlpha` on both color and alpha; feeding straight
//! alpha will produce visibly wrong compositing.
//!
//! ## Range overflow
//!
//! `Layer` uses `Range<u32>`, capping a single `Scene` at 4 G primitives per
//! kind. At 64 B/instance that's 256 GB — no realistic frame hits this.

use std::ops::Range;

use bytemuck::{Pod, Zeroable};

use crate::units::Lpx;

/// Truncate a `Vec::len()` to `u32`. The `Range<u32>` ranges in [`Layer`]
/// cap a single scene at 4 G primitives per kind (256 GB at 64 B / instance —
/// no realistic frame hits this); `debug_assert!` catches the boundary in
/// development builds.
#[inline]
fn len_as_u32(n: usize) -> u32 {
    debug_assert!(
        n <= u32::MAX as usize,
        "scene primitive count exceeds u32::MAX"
    );
    n as u32
}

// =====================================================================
// Instance structs (GPU-bound, #[repr(C)] + Pod + Zeroable)
// =====================================================================

/// Instance data for a filled, antialiased rounded rectangle.
///
/// Total size: **48 bytes**, 16-byte aligned.
///
/// `color` is **linear, premultiplied** RGBA; build via
/// [`crate::srgb_u8_to_linear_premul`].
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct RectInstance {
    /// `[x, y, w, h]` in logical pixels (top-left origin, +Y down).
    pub rect: [Lpx; 4],
    /// Linear RGBA, premultiplied (rgb already × a).
    pub color: [f32; 4],
    /// Corner radius in logical pixels (0 = sharp).
    pub corner_radius: Lpx,
    /// Explicit padding to reach 48-byte / 16-byte aligned layout.
    pub _pad: [f32; 3],
}
const _: () = assert!(core::mem::size_of::<RectInstance>() == 48);
const _: () = assert!(core::mem::align_of::<RectInstance>() == 4);

/// Instance data for a Gaussian-blurred drop shadow (Phase 6 pipeline).
///
/// Total size: **48 bytes**, 4-byte aligned (vertex-step instance buffers
/// have no std140 stride requirement; the plan doc's "64 bytes" comment was
/// an off-by-one — actual layout is 16+16+4+4+8 = 48).
///
/// `color` is **linear, premultiplied** RGBA. The shadow shader expands the
/// quad by `±3σ` from `rect`; callers do not pre-inflate.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ShadowInstance {
    /// Unexpanded `[x, y, w, h]` of the shadowed shape (logical pixels).
    pub rect: [Lpx; 4],
    /// Linear RGBA, premultiplied.
    pub color: [f32; 4],
    /// Corner radius of the shadowed shape, in logical pixels.
    pub corner_radius: Lpx,
    /// Gaussian σ (blur radius) in logical pixels.
    pub blur_radius: Lpx,
    /// Padding to 48-byte stride.
    pub _pad: [f32; 2],
}
const _: () = assert!(core::mem::size_of::<ShadowInstance>() == 48);

/// Instance data for an atlas-backed image quad (Phase 4 pipeline).
///
/// Total size: **48 bytes** (three packed `vec4<f32>` attributes — same
/// rationale as `ShadowInstance` re. plan doc's "64 bytes" comment).
///
/// `tint` is a **premultiplied linear** RGBA multiplier — `[1, 1, 1, 1]` is
/// the no-op identity (atlas sample passes through unchanged).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ImageInstance {
    /// Destination `[x, y, w, h]` on screen, in logical pixels.
    pub rect: [Lpx; 4],
    /// Source `[u0, v0, u1, v1]` rect in the atlas, normalized 0..1.
    pub uv_rect: [f32; 4],
    /// Linear RGBA premultiplied tint multiplier (white = no tint).
    pub tint: [f32; 4],
}
const _: () = assert!(core::mem::size_of::<ImageInstance>() == 48);

/// Instance data for a glyph quad sampled from the R8 alpha atlas
/// (Phase 5 pipeline).
///
/// Total size: **64 bytes**, 16-byte aligned.
///
/// `color` is the **premultiplied linear** ink color; the fragment shader
/// multiplies the sampled alpha mask by this color. Sub-pixel variant count
/// is locked at **4** (red-team P1-9 / Open Q #1 resolved); higher counts
/// would blow the single-page atlas cap.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct GlyphInstance {
    /// Destination `[x, y, w, h]` on screen, in logical pixels.
    pub rect: [Lpx; 4],
    /// Source `[u0, v0, u1, v1]` rect in the glyph atlas, normalized 0..1.
    pub uv_rect: [f32; 4],
    /// Linear RGBA premultiplied ink color (alpha-mask × this).
    pub color: [f32; 4],
    /// Sub-pixel positioning variant, range `0..4`.
    pub sub_pixel_variant: u32,
    /// Padding to 64-byte / 16-byte aligned layout.
    pub _pad: [u32; 3],
}
const _: () = assert!(core::mem::size_of::<GlyphInstance>() == 64);

// =====================================================================
// Layer
// =====================================================================

/// Half-open ranges into a [`Scene`]'s four primitive vectors, identifying
/// the slice of each that belongs to one painter's-algorithm layer.
///
/// `Range<u32>` (not `usize`) so the renderer can pass these directly to
/// `RenderPass::draw_indexed`/`draw` without truncation worry.
#[derive(Clone, Debug, Default)]
pub struct Layer {
    pub rects: Range<u32>,
    pub shadows: Range<u32>,
    pub images: Range<u32>,
    pub glyphs: Range<u32>,
}

impl Layer {
    /// New layer anchored at the supplied vec lengths (all four ranges
    /// `start..start`, i.e. empty).
    fn anchored(rect_len: u32, shadow_len: u32, image_len: u32, glyph_len: u32) -> Self {
        Self {
            rects: rect_len..rect_len,
            shadows: shadow_len..shadow_len,
            images: image_len..image_len,
            glyphs: glyph_len..glyph_len,
        }
    }
}

// =====================================================================
// Scene
// =====================================================================

/// Per-frame primitive container. Cleared between frames, filled by the
/// caller via `push_*` methods, drained by `Renderer::render` (which calls
/// `finish()` internally — callers must not).
///
/// Steady-state pushes do not allocate once each `Vec` reaches capacity:
/// `clear()` preserves capacity, and the only allocations are `Vec::push`
/// growth.
#[derive(Debug, Default)]
pub struct Scene {
    pub rects: Vec<RectInstance>,
    pub shadows: Vec<ShadowInstance>,
    pub images: Vec<ImageInstance>,
    pub glyphs: Vec<GlyphInstance>,
    pub layers: Vec<Layer>,
    /// Tracks whether the last entry in `layers` is still being filled. A
    /// `push_*` with no open layer auto-creates layer 0; `push_layer()`
    /// closes the open one and starts a new one.
    cur_layer_open: bool,
}

impl Scene {
    /// Create an empty scene with no allocations.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reset all four vecs and the layer list, preserving capacity for reuse.
    pub fn clear(&mut self) {
        self.rects.clear();
        self.shadows.clear();
        self.images.clear();
        self.glyphs.clear();
        self.layers.clear();
        self.cur_layer_open = false;
    }

    /// Start a new layer; subsequent `push_*` calls extend it.
    ///
    /// Closes the previous layer (its `end` indices already reflect every
    /// push since it was opened — closing is just a bookkeeping flip).
    ///
    /// Consecutive `push_layer` calls with no intervening primitive collapse
    /// to a single empty layer (we don't accumulate zero-sized layers — the
    /// renderer would just skip them).
    pub fn push_layer(&mut self) {
        if self.cur_layer_open
            && let Some(top) = self.layers.last()
            && Self::layer_is_empty(top)
        {
            // Already sitting on an empty open layer — nothing to do.
            return;
        }
        self.layers.push(Layer::anchored(
            len_as_u32(self.rects.len()),
            len_as_u32(self.shadows.len()),
            len_as_u32(self.images.len()),
            len_as_u32(self.glyphs.len()),
        ));
        self.cur_layer_open = true;
    }

    fn layer_is_empty(l: &Layer) -> bool {
        l.rects.is_empty() && l.shadows.is_empty() && l.images.is_empty() && l.glyphs.is_empty()
    }

    pub fn push_rect(&mut self, inst: RectInstance) {
        self.ensure_open_layer();
        self.rects.push(inst);
        // ensure_open_layer guarantees layers is non-empty.
        self.layers.last_mut().unwrap().rects.end = len_as_u32(self.rects.len());
    }

    pub fn push_shadow(&mut self, inst: ShadowInstance) {
        self.ensure_open_layer();
        self.shadows.push(inst);
        self.layers.last_mut().unwrap().shadows.end = len_as_u32(self.shadows.len());
    }

    pub fn push_image(&mut self, inst: ImageInstance) {
        self.ensure_open_layer();
        self.images.push(inst);
        self.layers.last_mut().unwrap().images.end = len_as_u32(self.images.len());
    }

    pub fn push_glyph(&mut self, inst: GlyphInstance) {
        self.ensure_open_layer();
        self.glyphs.push(inst);
        self.layers.last_mut().unwrap().glyphs.end = len_as_u32(self.glyphs.len());
    }

    /// Close the currently-open layer. Called by `Renderer::render_scene`
    /// before drawing, and by `HeadlessApp::render` for offscreen rendering.
    ///
    /// Idempotent: a no-op if no layer is open.
    pub fn finish(&mut self) {
        self.cur_layer_open = false;

        #[cfg(debug_assertions)]
        {
            let r = self.rects.len() as u32;
            let s = self.shadows.len() as u32;
            let i = self.images.len() as u32;
            let g = self.glyphs.len() as u32;
            let mut prev = (0u32, 0u32, 0u32, 0u32);
            for (n, l) in self.layers.iter().enumerate() {
                debug_assert!(
                    l.rects.start >= prev.0
                        && l.rects.end <= r
                        && l.shadows.start >= prev.1
                        && l.shadows.end <= s
                        && l.images.start >= prev.2
                        && l.images.end <= i
                        && l.glyphs.start >= prev.3
                        && l.glyphs.end <= g,
                    "Layer {n} range out of bounds: {l:?} (vec lens rects={r} shadows={s} images={i} glyphs={g})"
                );
                prev = (l.rects.end, l.shadows.end, l.images.end, l.glyphs.end);
            }
        }
    }

    fn ensure_open_layer(&mut self) {
        if !self.cur_layer_open {
            self.push_layer();
        }
    }
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_rect() -> RectInstance {
        RectInstance {
            rect: [Lpx(0.0), Lpx(0.0), Lpx(10.0), Lpx(10.0)],
            color: [0.5, 0.5, 0.5, 0.5],
            corner_radius: Lpx(0.0),
            _pad: [0.0; 3],
        }
    }

    fn dummy_shadow() -> ShadowInstance {
        ShadowInstance {
            rect: [Lpx(0.0), Lpx(0.0), Lpx(10.0), Lpx(10.0)],
            color: [0.0, 0.0, 0.0, 0.5],
            corner_radius: Lpx(0.0),
            blur_radius: Lpx(4.0),
            _pad: [0.0; 2],
        }
    }

    fn dummy_image() -> ImageInstance {
        ImageInstance {
            rect: [Lpx(0.0); 4],
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            tint: [1.0; 4],
        }
    }

    fn dummy_glyph() -> GlyphInstance {
        GlyphInstance {
            rect: [Lpx(0.0); 4],
            uv_rect: [0.0, 0.0, 1.0, 1.0],
            color: [1.0; 4],
            sub_pixel_variant: 0,
            _pad: [0; 3],
        }
    }

    #[test]
    fn push_rect_without_layer_creates_layer_0() {
        let mut s = Scene::new();
        s.push_rect(dummy_rect());
        assert_eq!(s.layers.len(), 1);
        assert_eq!(s.layers[0].rects, 0..1);
        assert_eq!(s.layers[0].shadows, 0..0);
    }

    #[test]
    fn multiple_layers_disjoint_ranges() {
        let mut s = Scene::new();
        s.push_rect(dummy_rect());
        s.push_rect(dummy_rect());
        s.push_layer();
        s.push_rect(dummy_rect());
        s.push_rect(dummy_rect());
        s.push_rect(dummy_rect());
        assert_eq!(s.layers.len(), 2);
        assert_eq!(s.layers[0].rects, 0..2);
        assert_eq!(s.layers[1].rects, 2..5);
    }

    #[test]
    fn clear_resets_lengths_keeps_capacity() {
        let mut s = Scene::new();
        for _ in 0..16 {
            s.push_rect(dummy_rect());
        }
        let cap_before = s.rects.capacity();
        s.clear();
        assert_eq!(s.rects.len(), 0);
        assert_eq!(s.layers.len(), 0);
        assert!(s.rects.capacity() >= cap_before);
        // After clear, next push must still implicitly open layer 0.
        s.push_rect(dummy_rect());
        assert_eq!(s.layers.len(), 1);
        assert_eq!(s.layers[0].rects, 0..1);
    }

    #[test]
    fn finish_closes_open_layer_idempotent() {
        let mut s = Scene::new();
        s.push_rect(dummy_rect());
        s.finish();
        s.finish();
        // Subsequent push opens a fresh layer (the previous one is closed).
        s.push_rect(dummy_rect());
        assert_eq!(s.layers.len(), 2);
        assert_eq!(s.layers[0].rects, 0..1);
        assert_eq!(s.layers[1].rects, 1..2);
    }

    #[test]
    fn instance_struct_sizes_match_const_assert() {
        assert_eq!(core::mem::size_of::<RectInstance>(), 48);
        // Shadow + Image are 48 (plan doc had a stale "64 bytes" comment —
        // the field lists in plan.md add to 48). Glyph is 64.
        assert_eq!(core::mem::size_of::<ShadowInstance>(), 48);
        assert_eq!(core::mem::size_of::<ImageInstance>(), 48);
        assert_eq!(core::mem::size_of::<GlyphInstance>(), 64);
    }

    #[test]
    fn consecutive_push_layer_collapses_to_one_empty_layer() {
        // Reviewer concern: back-to-back push_layer() shouldn't accumulate
        // empty layers (the renderer would just skip them, but the bookkeeping
        // is misleading). First call opens; subsequent calls on an empty open
        // layer no-op.
        let mut s = Scene::new();
        s.push_layer();
        s.push_layer();
        s.push_layer();
        assert_eq!(s.layers.len(), 1);

        // After at least one push, push_layer should add a fresh layer again.
        s.push_rect(dummy_rect());
        s.push_layer();
        assert_eq!(s.layers.len(), 2);
        assert_eq!(s.layers[0].rects, 0..1);
        assert_eq!(s.layers[1].rects, 1..1);

        // Repeating push_layer on the (now empty) second layer collapses.
        s.push_layer();
        assert_eq!(s.layers.len(), 2);
    }

    #[test]
    fn each_primitive_kind_extends_only_its_own_range() {
        let mut s = Scene::new();
        s.push_rect(dummy_rect());
        s.push_shadow(dummy_shadow());
        s.push_image(dummy_image());
        s.push_glyph(dummy_glyph());
        assert_eq!(s.layers.len(), 1);
        let l = &s.layers[0];
        assert_eq!(l.rects, 0..1);
        assert_eq!(l.shadows, 0..1);
        assert_eq!(l.images, 0..1);
        assert_eq!(l.glyphs, 0..1);
    }
}
