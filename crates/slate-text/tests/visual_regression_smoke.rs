//! Smoke tests for the visual regression harness.
//!
//! These tests validate the golden comparison infrastructure before any
//! real text rendering code is added.

mod common;

use common::{assert_alpha_matches_golden, assert_rgba_matches_golden, update_golden_mode};
use wgpu::{
    BufferDescriptor, BufferUsages, Color, CommandEncoderDescriptor, Extent3d, LoadOp, MapMode,
    Operations, PollType, RenderPassColorAttachment, RenderPassDescriptor, StoreOp,
    TexelCopyBufferInfo, TexelCopyBufferLayout, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages, TextureViewDescriptor,
};

/// Test that alpha bitmap golden comparison works.
#[test]
fn round_trip_alpha_golden() {
    let width = 16u32;
    let height = 16u32;
    let mut pixels = vec![0u8; (width * height) as usize];

    for y in 0..height {
        for x in 0..width {
            if x == y {
                pixels[(y * width + x) as usize] = 255;
            }
        }
    }

    let golden_path = common::golden::golden_path("diagonal-alpha");
    if !update_golden_mode() && !golden_path.exists() {
        eprintln!(
            "Skipping round_trip_alpha_golden: golden missing at {}. Run with UPDATE_GOLDENS=1.",
            golden_path.display()
        );
        return;
    }

    assert_alpha_matches_golden(&pixels, width, height, "diagonal-alpha");
}

/// Test that RGBA golden comparison works via GPU render-to-texture.
#[test]
fn round_trip_rgba_via_render_to_texture() {
    let Some((device, queue)) = common::make_headless_device() else {
        eprintln!("Skipping round_trip_rgba_via_render_to_texture: no GPU adapter available");
        return;
    };

    let width = 32u32;
    let height = 32u32;
    let format = TextureFormat::Rgba8Unorm;

    let target = device.create_texture(&TextureDescriptor {
        label: Some("smoke-test-target"),
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

    let unpadded_bytes_per_row = width * 4;
    let padded_bytes_per_row = unpadded_bytes_per_row.div_ceil(256) * 256;
    let buffer_size = (padded_bytes_per_row as u64) * (height as u64);

    let readback = device.create_buffer(&BufferDescriptor {
        label: Some("smoke-test-readback"),
        size: buffer_size,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("smoke-test-encoder"),
    });

    {
        let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("smoke-test-pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(Color {
                        r: 1.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    }),
                    store: StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }

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
        .expect("device.poll failed");

    let mapped = slice.get_mapped_range();
    let mut pixels = vec![0u8; (unpadded_bytes_per_row * height) as usize];
    for row in 0..height as usize {
        let src_off = row * padded_bytes_per_row as usize;
        let dst_off = row * unpadded_bytes_per_row as usize;
        pixels[dst_off..dst_off + unpadded_bytes_per_row as usize]
            .copy_from_slice(&mapped[src_off..src_off + unpadded_bytes_per_row as usize]);
    }
    drop(mapped);
    readback.unmap();

    let golden_path = common::golden::golden_path("clear-red");
    if !update_golden_mode() && !golden_path.exists() {
        eprintln!(
            "Skipping round_trip_rgba_via_render_to_texture: golden missing at {}. Run with UPDATE_GOLDENS=1.",
            golden_path.display()
        );
        return;
    }

    assert_rgba_matches_golden(&pixels, width, height, "clear-red");
}
