//! Regression lock for the second `LayerOrdering` impl (`DepthBucketOrder`).
//!
//! Two checks pin the contract:
//!
//!   1. When every layer's depth is zero, the walk is byte-identical to
//!      `DefaultPainterOrder` (stable sort preserves push order among
//!      equal-depth layers). This guarantees a no-op default-equivalence
//!      when the depth bucket strategy is wired but no depths are set.
//!
//!   2. When depths vary, layers are walked lowest-depth-first while the
//!      within-layer primitive sequence (shadows → rects → glyphs → images)
//!      is preserved.

use slate_renderer::{DefaultPainterOrder, DepthBucketOrder, LayerOrdering, ScenePrimitive};

fn collect(order: &impl LayerOrdering, layer_count: usize) -> Vec<(usize, ScenePrimitive)> {
    let mut seq = Vec::new();
    order.for_each_draw(layer_count, |layer, kind| seq.push((layer, kind)));
    seq
}

#[test]
fn all_zero_depths_match_default_painter_order() {
    let depths = vec![0; 4];
    let bucket = DepthBucketOrder::new(&depths);
    assert_eq!(collect(&bucket, 4), collect(&DefaultPainterOrder, 4));
}

#[test]
fn varied_depths_walk_ascending() {
    // Depths [2, 0, 1] → expected layer walk order is [1, 2, 0].
    let depths = [2, 0, 1];
    let bucket = DepthBucketOrder::new(&depths);
    let seq = collect(&bucket, 3);

    assert_eq!(
        seq,
        vec![
            // depth 0 → layer 1 first
            (1, ScenePrimitive::Shadow),
            (1, ScenePrimitive::Rect),
            (1, ScenePrimitive::Glyph),
            (1, ScenePrimitive::Image),
            // depth 1 → layer 2 second
            (2, ScenePrimitive::Shadow),
            (2, ScenePrimitive::Rect),
            (2, ScenePrimitive::Glyph),
            (2, ScenePrimitive::Image),
            // depth 2 → layer 0 last
            (0, ScenePrimitive::Shadow),
            (0, ScenePrimitive::Rect),
            (0, ScenePrimitive::Glyph),
            (0, ScenePrimitive::Image),
        ]
    );
}

#[test]
fn missing_depth_entries_default_to_zero() {
    // Only the first two layers have explicit depths; the third defaults to 0
    // and the stable sort keeps it in push order among the depth-0 group.
    let depths = [5, 0];
    let bucket = DepthBucketOrder::new(&depths);
    let seq = collect(&bucket, 3);

    // Walk order is [1 (0), 2 (0 default), 0 (5)].
    let layer_order: Vec<usize> = seq
        .iter()
        .filter_map(|(layer, kind)| (*kind == ScenePrimitive::Shadow).then_some(*layer))
        .collect();
    assert_eq!(layer_order, vec![1, 2, 0]);
}

#[test]
fn zero_layers_emits_nothing() {
    let depths: [i32; 0] = [];
    let bucket = DepthBucketOrder::new(&depths);
    let mut count = 0;
    bucket.for_each_draw(0, |_, _| count += 1);
    assert_eq!(count, 0);
}
