//! Packing-density measurement for the etagere shelf allocator.
//!
//! The atlas is already backed by `etagere::AtlasAllocator` — there is no
//! library to swap in. The open question is whether the *current* shelf
//! configuration packs densely enough, or whether `AtlasAllocatorOptions`
//! needs retuning. This test measures occupancy at out-of-space onset and
//! pins a floor so a future config regression (or a gutter change) that
//! degrades packing fails loudly.
//!
//! ## Measured verdict (2026-05-21, this machine) — KEEP current config
//!
//! - **Uniform 256² (the dense-gallery's real workload):** 49 tiles (7×7 of
//!   258²-padded), **~77.8% footprint occupancy** before `OutOfSpace`. Above
//!   the plan's 75% bar → no retune.
//! - **Mixed {256,192,128,96}² interleaved (adversarial):** ~92 tiles,
//!   **~71.7%**. The shortfall is intrinsic shelf-height-mixing waste — a
//!   258-tall shelf holding shorter tiles wastes the vertical gap above them.
//!   `AtlasAllocatorOptions` tunes shelf alignment, not the fundamental
//!   mixed-height shelf model, so it cannot recover this without a different
//!   packer — out of scope and YAGNI for the uniform-tile real consumer.
//!
//! Decision: keep the default `AtlasAllocator::new` config. The real workload
//! clears the bar; the adversarial case is a documented shelf-packer property,
//! not a config defect.

mod common;

use slate_renderer::allocate_image;
use slate_renderer::atlas::{Atlas, AtlasError, Format, PAGE_SIZE};

/// Fill a fresh single-frame atlas with images produced by `next_size` until
/// `OutOfSpace`, returning `(count, footprint_occupancy)`. Single frame means
/// nothing is touched-this-frame, so eviction is rejected and a full page
/// reports `OutOfSpace` — the occupancy-at-onset point.
fn measure_occupancy(device: &wgpu::Device, mut next_size: impl FnMut(u32) -> u32) -> (u32, f64) {
    let mut atlas = Atlas::new(device, Format::Rgba8UnormSrgb);
    atlas.begin_frame();
    let mut count = 0u32;
    loop {
        let s = next_size(count);
        match allocate_image(&mut atlas, s, s) {
            Ok(_) => count += 1,
            Err(AtlasError::OutOfSpace) => break,
            Err(e) => panic!("unexpected atlas error during packing spike: {e:?}"),
        }
        assert!(
            count < 2000,
            "page never reported OutOfSpace — packing broken"
        );
    }
    let page_area = (PAGE_SIZE as u64) * (PAGE_SIZE as u64);
    let occupancy = atlas.allocated_pixels() as f64 / page_area as f64;
    (count, occupancy)
}

#[test]
fn dense_image_workload_packs_above_floor() {
    let Some((device, _queue)) = common::make_headless_device() else {
        eprintln!("packing_density_spike: no GPU adapter — skipping");
        return;
    };

    // Primary: the dense-gallery's real workload — uniform 256² tiles.
    let (uniform_n, uniform_occ) = measure_occupancy(&device, |_| 256);
    eprintln!(
        "packing-density uniform 256²: {uniform_n} tiles = {:.1}%",
        uniform_occ * 100.0
    );

    // Context: an adversarial mixed-height interleave (documented in the header).
    const SIZES: [u32; 4] = [256, 192, 128, 96];
    let (mixed_n, mixed_occ) = measure_occupancy(&device, |c| SIZES[(c as usize) % SIZES.len()]);
    eprintln!(
        "packing-density mixed: {mixed_n} tiles = {:.1}%",
        mixed_occ * 100.0
    );

    // Plan criterion: keep current config while the real workload occupancy
    // is ≥ ~75%. Floor at 0.75 so a packing regression on the actual consumer
    // fails loudly; the adversarial mixed case is informational only.
    assert!(
        uniform_occ >= 0.75,
        "uniform-tile packing dropped to {:.1}% (< 75% bar) — investigate AtlasAllocatorOptions",
        uniform_occ * 100.0
    );
}
