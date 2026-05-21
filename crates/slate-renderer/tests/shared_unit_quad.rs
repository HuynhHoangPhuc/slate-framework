//! Regression guard for the single shared unit-quad vertex buffer.
//!
//! The renderer builds ONE `unit_quad` and ONE viewport bind-group layout and
//! hands them by reference into all four instanced pipelines (rect, shadow,
//! image, glyph) — there is no per-pipeline copy. That single-construction
//! invariant is enforced at compile time: every pipeline constructor takes
//! `&viewport_bgl` and every `record` takes `&unit_quad` by reference, so a
//! pipeline cannot own its own copy. This test pins the quad's fixed shape
//! (6 vertices × `[f32; 2]` = 48 bytes) so a future change to the shared quad
//! is a deliberate, visible edit rather than silent divergence.

mod common;

use slate_renderer::create_unit_quad;

#[test]
fn unit_quad_is_the_fixed_48_byte_six_vertex_buffer() {
    let Some((device, _queue)) = common::make_headless_device() else {
        eprintln!("shared_unit_quad: no GPU adapter — skipping");
        return;
    };
    // 6 vertices (two triangles, no index buffer) × 2 f32 × 4 bytes = 48.
    let quad = create_unit_quad(&device);
    assert_eq!(
        quad.size(),
        48,
        "shared unit quad must stay 6 vertices × vec2<f32>; a size change means \
         the quad shape diverged from every pipeline's @location(0) contract"
    );
}
