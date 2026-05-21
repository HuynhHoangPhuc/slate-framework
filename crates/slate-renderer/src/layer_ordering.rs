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
