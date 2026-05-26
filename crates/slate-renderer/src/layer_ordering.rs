//! The z-order seam: a trait that drives the per-frame draw walk so the
//! ordering strategy can be swapped post-v1 (depth buckets, BoundsTree)
//! without touching the pass-recording code in the renderer.

/// One drawable primitive class within a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScenePrimitive {
    Shadow,
    Rect,
    Glyph,
    Image,
}

/// Strategy for the order in which a scene's layers and their primitives are
/// recorded into the render pass. The renderer drives every draw through the
/// sequence this yields, so a different strategy needs no change to the
/// pass-recording code.
///
/// The v1 production default is [`DefaultPainterOrder`]. A second concrete
/// impl, [`DepthBucketOrder`], exists to prove the seam is genuinely
/// swappable and to back a regression-lock test against drift in the trait
/// shape.
pub trait LayerOrdering {
    /// Invoke `step` once per `(layer index, primitive)` draw, in record order.
    fn for_each_draw<F: FnMut(usize, ScenePrimitive)>(&self, layer_count: usize, step: F);
}

/// The v1 painter's-algorithm walk: layers in push order, and within each
/// layer the fixed shadows → rects → glyphs → images order.
pub struct DefaultPainterOrder;

impl LayerOrdering for DefaultPainterOrder {
    fn for_each_draw<F: FnMut(usize, ScenePrimitive)>(&self, layer_count: usize, mut step: F) {
        for layer in 0..layer_count {
            step(layer, ScenePrimitive::Shadow);
            step(layer, ScenePrimitive::Rect);
            step(layer, ScenePrimitive::Glyph);
            step(layer, ScenePrimitive::Image);
        }
    }
}

/// Second concrete `LayerOrdering` impl: groups layers by an explicit depth
/// and walks them lowest-depth-first. Within each layer the within-layer
/// primitive sequence is the same as [`DefaultPainterOrder`] (shadows →
/// rects → glyphs → images).
///
/// Exists to prove the [`LayerOrdering`] seam is genuinely swappable without
/// touching the renderer's pass-recording code, and to give the regression
/// suite a second target. The production default stays
/// [`DefaultPainterOrder`]; this impl has no production callsite.
pub struct DepthBucketOrder<'a> {
    depths: &'a [i32],
}

impl<'a> DepthBucketOrder<'a> {
    /// Construct from a slice of per-layer depths parallel to the scene's
    /// `Vec<Layer>`. Missing entries (i.e. `depths.len() < layer_count`)
    /// are treated as depth `0`.
    pub fn new(depths: &'a [i32]) -> Self {
        Self { depths }
    }
}

impl<'a> LayerOrdering for DepthBucketOrder<'a> {
    fn for_each_draw<F: FnMut(usize, ScenePrimitive)>(&self, layer_count: usize, mut step: F) {
        // Stable sort preserves push order among equal-depth layers, so an
        // all-zero `depths` walk matches `DefaultPainterOrder` byte-for-byte.
        let mut indices: Vec<usize> = (0..layer_count).collect();
        indices.sort_by_key(|&i| self.depths.get(i).copied().unwrap_or(0));
        for layer in indices {
            step(layer, ScenePrimitive::Shadow);
            step(layer, ScenePrimitive::Rect);
            step(layer, ScenePrimitive::Glyph);
            step(layer, ScenePrimitive::Image);
        }
    }
}
