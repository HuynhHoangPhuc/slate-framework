//! Shared headless-render test harness for slate-renderer integration tests.
//!
//! Each integration test file (e.g. `tests/smoke_premul.rs`) brings this in
//! with `mod common;` and calls [`make_headless_device`] + [`render_to_texture`]
//! + [`assert_pixel`].
//!
//! # Readback semantics
//!
//! The bytes returned by [`render_to_texture`] are the raw GPU readback from
//! whichever `format` you pass in, with the **byte order rewritten to RGBA**
//! when the source format is BGRA. For an `Bgra8UnormSrgb` surface the
//! channel encodings are:
//!
//! - **R / G / B**: 8-bit sRGB-encoded (the GPU did `linear → sRGB` on store)
//! - **A**: 8-bit linear passthrough (Unorm formats don't apply transfer to alpha)
//!
//! Tests that compute expected values in linear space MUST apply
//! `linear_to_srgb` to the colour channels before passing them to
//! [`assert_pixel`]. We deliberately do **not** decode in the harness — every
//! test should be explicit about which colour space it's asserting in.
//!
//! # Why pollster + sync interface
//!
//! `wgpu::Instance::request_adapter` and `request_device` are async; tests are
//! `#[test]` (sync). Each public helper that calls them blocks via
//! `pollster::block_on` so callers can stay in plain `fn`.

use wgpu::{
    Backends, BufferDescriptor, BufferUsages, CommandEncoderDescriptor, Device, DeviceDescriptor,
    ExperimentalFeatures, Extent3d, Features, Instance, InstanceDescriptor, Limits, MapMode,
    MemoryHints, PollType, Queue, RequestAdapterOptions, TexelCopyBufferInfo,
    TexelCopyBufferLayout, Texture, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages, TextureView, TextureViewDescriptor, Trace,
};

/// Round `n` up to the next multiple of `align`. Used for the 256-byte
/// `COPY_BYTES_PER_ROW_ALIGNMENT` contract on `Queue::copy_texture_to_buffer`.
fn align_up(n: u32, align: u32) -> u32 {
    n.div_ceil(align) * align
}

/// Acquire a wgpu device + queue with no surface attached.
///
/// Returns `None` if no adapter is available (headless CI on a runner without
/// a software adapter — e.g. some Windows CI images). Tests should treat
/// `None` as "skip" rather than fail.
pub fn make_headless_device() -> Option<(Device, Queue)> {
    pollster::block_on(make_headless_device_async())
}

async fn make_headless_device_async() -> Option<(Device, Queue)> {
    let instance = Instance::new(InstanceDescriptor {
        backends: Backends::PRIMARY,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: Default::default(),
        display: None,
    });

    let adapter = instance
        .request_adapter(&RequestAdapterOptions {
            // LowPower keeps llvmpipe / lavapipe / WARP happy on CI runners.
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .ok()?;

    let (device, queue) = adapter
        .request_device(&DeviceDescriptor {
            label: Some("slate-headless-test-device"),
            required_features: Features::empty(),
            required_limits: Limits::downlevel_defaults(),
            memory_hints: MemoryHints::Performance,
            trace: Trace::Off,
            experimental_features: ExperimentalFeatures::disabled(),
        })
        .await
        .ok()?;

    Some((device, queue))
}

/// Render into a fresh off-screen texture and copy it back to a `Vec<u8>`.
///
/// `encode` receives an open `CommandEncoder` and a `TextureView` for the
/// off-screen render target. Implementations should `begin_render_pass` with
/// the supplied view, draw whatever they need, and let the encoder return —
/// this helper handles texture allocation, the copy-to-buffer step, the
/// 256-byte row-alignment dance, and the BGRA→RGBA byte swap on readback.
///
/// Returns a tightly packed `width * height * 4` byte buffer in **RGBA order**
/// regardless of the source `format` (we swap channels for BGRA formats).
pub fn render_to_texture<F>(
    device: &Device,
    queue: &Queue,
    format: TextureFormat,
    width: u32,
    height: u32,
    encode: F,
) -> Vec<u8>
where
    F: FnOnce(&mut wgpu::CommandEncoder, &TextureView),
{
    assert!(width > 0 && height > 0, "render_to_texture: zero size");

    let target: Texture = device.create_texture(&TextureDescriptor {
        label: Some("slate-headless-target"),
        size: Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&TextureViewDescriptor::default());

    // 256-byte row alignment for copy_texture_to_buffer (wgpu spec).
    let unpadded_bytes_per_row = width * 4;
    let padded_bytes_per_row = align_up(unpadded_bytes_per_row, 256);
    let buffer_size = (padded_bytes_per_row as u64) * (height as u64);

    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("slate-headless-readback"),
        size: buffer_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("slate-headless-encoder"),
    });

    encode(&mut encoder, &view);

    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        TexelCopyBufferInfo {
            buffer: &readback,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_bytes_per_row),
                rows_per_image: Some(height),
            },
        },
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(std::iter::once(encoder.finish()));

    let slice = readback.slice(..);
    slice.map_async(MapMode::Read, |_| {});
    device
        .poll(PollType::wait_indefinitely())
        .expect("device.poll(Wait) should not fail on headless test device");

    let mapped = slice.get_mapped_range();

    // Strip row padding into a tight RGBA buffer.
    let mut out = vec![0u8; (unpadded_bytes_per_row as usize) * (height as usize)];
    for row in 0..height as usize {
        let src_off = row * padded_bytes_per_row as usize;
        let dst_off = row * unpadded_bytes_per_row as usize;
        out[dst_off..dst_off + unpadded_bytes_per_row as usize]
            .copy_from_slice(&mapped[src_off..src_off + unpadded_bytes_per_row as usize]);
    }
    drop(mapped);
    readback.unmap();

    if matches!(
        format,
        TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb
    ) {
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }

    out
}

/// Assert that the pixel at `(x, y)` in an RGBA `width × ?` buffer matches
/// `expected` within `tolerance` per channel. Panics with a readable diff.
#[track_caller]
pub fn assert_pixel(buf: &[u8], width: u32, x: u32, y: u32, expected: [u8; 4], tolerance: u8) {
    let stride = width as usize * 4;
    let off = (y as usize) * stride + (x as usize) * 4;
    let actual = [buf[off], buf[off + 1], buf[off + 2], buf[off + 3]];
    for i in 0..4 {
        let diff = actual[i].abs_diff(expected[i]);
        if diff > tolerance {
            panic!(
                "pixel mismatch at ({x}, {y}) channel {i}: actual {actual:?}, expected {expected:?} (±{tolerance})"
            );
        }
    }
}
