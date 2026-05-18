//! Renderer viewport-uniform contract test.
//!
//! Locks in that the viewport buffer carries **lpx**, not physical pixels.
//! WGSL shader math `pixel_pos / viewport.size` is scale-invariant, so both
//! numerator and denominator must agree on units; the scene wire format is
//! lpx (see `scene_wire_format_lpx.rs`), so the viewport must be lpx too.
//!
//! `Renderer::new` is window-coupled (CAMetalLayer / DXGI swapchain) and
//! cannot be spun up headlessly. Instead we mirror the renderer's
//! viewport-write path exactly: same `ViewportUniform` layout, same
//! `bytes_of` upload, same `MAP_READ` readback — verifying the contract that
//! `logical_size` (not physical drawable size) lands in the buffer.

mod common;

use slate_renderer::{Lpx, ViewportUniform};
use wgpu::{BufferDescriptor, BufferUsages, MapMode, PollType};

#[test]
fn viewport_uniform_stores_lpx_not_physical() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("Skipping test: no GPU adapter available");
        return;
    };

    // Scenario: Retina display at scale=2.0.
    //   logical_size  = (800, 600)   ← lpx; what the renderer must write
    //   drawable_size = (1600, 1200) ← physical px; pre-fix value (must NOT appear)
    let logical_w: u32 = 800;
    let logical_h: u32 = 600;

    let size = std::mem::size_of::<ViewportUniform>() as u64;
    let upload = device.create_buffer(&BufferDescriptor {
        label: Some("test-viewport-upload"),
        size,
        usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST | BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("test-viewport-readback"),
        size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    // Mirrors Renderer::new (viewport_buf write) and Renderer::resize.
    queue.write_buffer(
        &upload,
        0,
        bytemuck::bytes_of(&ViewportUniform {
            size: [Lpx(logical_w as f32), Lpx(logical_h as f32)],
            _pad: [0.0; 2],
        }),
    );

    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_buffer_to_buffer(&upload, 0, &readback, 0, size);
    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(MapMode::Read, move |r| {
        tx.send(r).ok();
    });
    let _ = device.poll(PollType::wait_indefinitely());
    rx.recv().unwrap().unwrap();

    let data = slice.get_mapped_range();
    let v: &ViewportUniform = bytemuck::from_bytes(&data);
    assert_eq!(
        v.size,
        [Lpx(800.0), Lpx(600.0)],
        "viewport must store lpx (800x600), not physical px (1600x1200)"
    );
    assert_ne!(
        v.size,
        [Lpx(1600.0), Lpx(1200.0)],
        "viewport stored physical-pixel size — Bug 2 regression"
    );
}
