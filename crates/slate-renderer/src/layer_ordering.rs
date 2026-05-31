//! The z-order seam: a trait that drives the per-frame draw walk so the
//! ordering strategy can be swapped post-v1 (depth buckets, BoundsTree)
//! without touching the pass-recording code in the renderer.

/// One drawable primitive class within a layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScenePrimitive {
    /// Box-shadow primitive (drawn first, behind everything else).
    Shadow,
    /// Solid / gradient rectangle primitive.
    Rect,
    /// Text glyph primitive.
    Glyph,
    /// Raster image primitive.
    Image,
}

/// Stable depth-sort of layer indices: returns `0..depths.len()` ordered by
/// ascending `depths[i]`, ties broken by original push order.
///
/// This is the shared ordering primitive both record sites use — the windowed
/// submit path and the headless golden path — so depth ordering is identical on
/// screen and in golden images. An all-`0` `depths` slice yields `[0, 1, 2, …]`
/// unchanged — the byte-for-byte regression lock against the pre-depth
/// push-order walk.
///
/// Callers pass a slice exactly as long as the scene's layer list. Padding a
/// short slice to a layer count (missing entries → depth `0`) is the caller's
/// job; see [`DepthBucketOrder::for_each_draw`].
pub fn depth_sorted_layers(depths: &[i32]) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..depths.len()).collect();
    // Stable sort preserves push order among equal-depth layers.
    indices.sort_by_key(|&i| depths[i]);
    indices
}

/// Strategy for the order in which a scene's layers and their primitives are
/// recorded into the render pass. The renderer drives every draw through the
/// sequence this yields, so a different strategy needs no change to the
/// pass-recording code.
///
/// The production order is [`DepthBucketOrder`], which walks layers
/// lowest-depth-first so overlays/popovers (high depth) draw on top.
/// [`DefaultPainterOrder`] is kept as the semantic reference (plain push order)
/// and as a regression-lock target: an all-depth-`0` `DepthBucketOrder` walk
/// must match it byte-for-byte.
pub trait LayerOrdering {
    /// Invoke `step` once per `(layer index, primitive)` draw, in record order.
    fn for_each_draw<F: FnMut(usize, ScenePrimitive)>(&self, layer_count: usize, step: F);
}

/// The painter's-algorithm reference walk: layers in push order, and within
/// each layer the fixed shadows → rects → glyphs → images order. Production
/// rendering uses [`DepthBucketOrder`]; this is retained as the semantic
/// reference and regression-lock baseline (an all-depth-`0` depth walk equals
/// this).
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

/// The production `LayerOrdering` impl: groups layers by an explicit per-layer
/// depth (`Layer::depth`) and walks them lowest-depth-first. Within each layer
/// the primitive sequence is the same as [`DefaultPainterOrder`] (shadows →
/// rects → glyphs → images).
///
/// Lowest-depth-first with a stable tie-break (push order) is what lets an
/// overlay declared deep in the tree paint above everything below it: it pushes
/// a layer at a high depth, and this walk records that layer last regardless of
/// where it sits in the push sequence. An all-depth-`0` scene walks in plain
/// push order — byte-identical to [`DefaultPainterOrder`].
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
        // Pad/truncate the depth view to `layer_count` so callers may pass a
        // depths slice that is shorter (missing entries → depth 0). Sharing
        // `depth_sorted_layers` keeps this walk identical to the headless path.
        let depths: Vec<i32> = (0..layer_count)
            .map(|i| self.depths.get(i).copied().unwrap_or(0))
            .collect();
        for layer in depth_sorted_layers(&depths) {
            step(layer, ScenePrimitive::Shadow);
            step(layer, ScenePrimitive::Rect);
            step(layer, ScenePrimitive::Glyph);
            step(layer, ScenePrimitive::Image);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_zero_depths_preserve_push_order() {
        // The regression lock: a scene where every layer is depth 0 must walk
        // in plain push order, matching the pre-depth behaviour byte-for-byte.
        assert_eq!(depth_sorted_layers(&[0, 0, 0, 0]), vec![0, 1, 2, 3]);
    }

    #[test]
    fn higher_depth_sorts_last() {
        // Layer 0 at depth 10 (an overlay) must record after the depth-0 base
        // layers, even though it was pushed first.
        assert_eq!(depth_sorted_layers(&[10, 0, 0]), vec![1, 2, 0]);
    }

    #[test]
    fn equal_depths_keep_push_order_stable() {
        // Among equal depths the original push order is preserved (stable sort).
        assert_eq!(depth_sorted_layers(&[5, 1, 5, 1]), vec![1, 3, 0, 2]);
    }

    #[test]
    fn negative_depths_sort_below_zero() {
        assert_eq!(depth_sorted_layers(&[0, -3, 2]), vec![1, 0, 2]);
    }

    #[test]
    fn empty_scene_walks_nothing() {
        assert!(depth_sorted_layers(&[]).is_empty());
    }

    #[test]
    fn depth_bucket_order_matches_default_when_all_zero() {
        // Drive both orderings over a 3-layer all-zero scene and assert the
        // emitted (layer, primitive) sequences are identical — the trait-level
        // regression lock.
        let mut depth_seq = Vec::new();
        DepthBucketOrder::new(&[0, 0, 0]).for_each_draw(3, |l, k| depth_seq.push((l, k)));
        let mut default_seq = Vec::new();
        DefaultPainterOrder.for_each_draw(3, |l, k| default_seq.push((l, k)));
        assert_eq!(depth_seq, default_seq);
    }

    #[test]
    fn depth_bucket_order_pads_short_depths_slice() {
        // A depths slice shorter than layer_count treats the tail as depth 0.
        let mut seq = Vec::new();
        DepthBucketOrder::new(&[5]).for_each_draw(3, |l, _| {
            if !seq.contains(&l) {
                seq.push(l);
            }
        });
        // Layers 1 and 2 (depth 0) record before layer 0 (depth 5).
        assert_eq!(seq, vec![1, 2, 0]);
    }
}
