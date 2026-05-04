//! Headless smoke test for [`RectPipeline`].
//!
//! Initialises a wgpu `Instance` + `Adapter` without a surface, builds a
//! `RectPipeline` against an offscreen `Bgra8UnormSrgb` texture, draws a
//! centered red rectangle, reads back the pixel data, and asserts that the
//! center pixel is dominated by the red channel.
//!
//! Run with:
//! ```sh
//! cargo run --example rect_smoke -p slate-renderer
//! ```
//!
//! If no GPU adapter is available (CI / pure-software environment), the test
//! exits with a clear diagnostic message and a non-zero code — this is
//! acceptable per spec.  The binary must always *compile* cleanly.

use wgpu::{
    Backends, BufferDescriptor, BufferUsages, Color, CommandEncoderDescriptor, DeviceDescriptor,
    Extent3d, ExperimentalFeatures, Features, Instance, InstanceDescriptor, Limits, LoadOp,
    MemoryHints, Operations, Origin3d, PollType, RenderPassColorAttachment, RenderPassDescriptor,
    RequestAdapterOptions, StoreOp, TexelCopyBufferInfo, TexelCopyBufferLayout,
    TexelCopyTextureInfo, TextureAspect, TextureDescriptor, TextureDimension, TextureFormat,
    TextureUsages, TextureViewDescriptor, Trace,
};

use slate_renderer::{RectPipeline, RectUniform};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 256;
// Bgra8UnormSrgb: 4 bytes per pixel, row pitch must be a multiple of 256.
const BYTES_PER_PIXEL: u32 = 4;
const UNPADDED_ROW: u32 = WIDTH * BYTES_PER_PIXEL;
// wgpu requires `COPY_BYTES_PER_ROW_ALIGNMENT` (256) alignment for buffer copies.
const ALIGN: u32 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
const PADDED_ROW: u32 = UNPADDED_ROW.div_ceil(ALIGN) * ALIGN;

fn main() {
    pollster::block_on(run());
}

async fn run() {
    let instance = Instance::new(InstanceDescriptor {
        backends: Backends::PRIMARY,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: Default::default(),
        display: None,
    });

    // Request adapter without a surface (headless).
    let adapter = match instance
        .request_adapter(&RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
    {
        Ok(a) => a,
        Err(_) => {
            eprintln!("rect_smoke: no GPU adapter found — skipping (acceptable in headless CI)");
            std::process::exit(1);
        }
    };

    println!("rect_smoke: adapter = {:?}", adapter.get_info());

    let (device, queue) = adapter
        .request_device(&DeviceDescriptor {
            label: Some("rect-smoke-device"),
            required_features: Features::empty(),
            required_limits: Limits::downlevel_defaults(),
            memory_hints: MemoryHints::Performance,
            trace: Trace::Off,
            experimental_features: ExperimentalFeatures::disabled(),
        })
        .await
        .expect("failed to open GPU device");

    // Use Bgra8UnormSrgb as the offscreen render target format.
    let format = TextureFormat::Bgra8UnormSrgb;

    // Build the pipeline under test.
    let pipeline = RectPipeline::new(&device, format);

    // Offscreen render target.
    let render_texture = device.create_texture(&TextureDescriptor {
        label: Some("rect-smoke-rt"),
        size: Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: TextureDimension::D2,
        format,
        // RENDER_ATTACHMENT so we can draw into it; COPY_SRC so we can read it back.
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let render_view = render_texture.create_view(&TextureViewDescriptor::default());

    // Upload a centered red rectangle that fills most of the viewport.
    pipeline.upload(
        &queue,
        RectUniform {
            center: [WIDTH as f32 / 2.0, HEIGHT as f32 / 2.0],
            half_size: [80.0, 80.0],
            color: [1.0, 0.0, 0.0, 1.0], // red
            viewport_size: [WIDTH as f32, HEIGHT as f32],
            corner_radius: 8.0,
            _pad: 0.0,
        },
    );

    // Encode: clear to black + draw rect.
    let mut encoder = device.create_command_encoder(&CommandEncoderDescriptor {
        label: Some("rect-smoke-encoder"),
    });
    {
        let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
            label: Some("rect-smoke-pass"),
            color_attachments: &[Some(RenderPassColorAttachment {
                view: &render_view,
                depth_slice: None,
                resolve_target: None,
                ops: Operations {
                    load: LoadOp::Clear(Color::BLACK),
                    store: StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
            multiview_mask: None,
        });
        pipeline.record(&mut rpass);
    }

    // Readback buffer: padded rows, GPU → CPU.
    let readback_buf = device.create_buffer(&BufferDescriptor {
        label: Some("rect-smoke-readback"),
        size: (PADDED_ROW * HEIGHT) as u64,
        usage: BufferUsages::COPY_DST | BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    // Copy render texture → readback buffer.
    // wgpu 29: ImageCopyTexture → TexelCopyTextureInfo, ImageCopyBuffer → TexelCopyBufferInfo,
    //           ImageDataLayout → TexelCopyBufferLayout.
    encoder.copy_texture_to_buffer(
        TexelCopyTextureInfo {
            texture: &render_texture,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        TexelCopyBufferInfo {
            buffer: &readback_buf,
            layout: TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(PADDED_ROW),
                rows_per_image: Some(HEIGHT),
            },
        },
        Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );

    queue.submit(std::iter::once(encoder.finish()));

    // Map the readback buffer and inspect the center pixel.
    let slice = readback_buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        tx.send(result).unwrap();
    });
    // wgpu 29: Maintain::Wait → PollType::Wait { submission_index: None, timeout: None }.
    device
        .poll(PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll failed");
    rx.recv()
        .expect("channel closed")
        .expect("buffer map failed");

    let data = slice.get_mapped_range();

    // Center pixel: row = HEIGHT/2, col = WIDTH/2.
    // Format is Bgra8UnormSrgb → bytes are [B, G, R, A].
    let row = HEIGHT / 2;
    let col = WIDTH / 2;
    let byte_offset = (row * PADDED_ROW + col * BYTES_PER_PIXEL) as usize;
    let b = data[byte_offset];
    let g = data[byte_offset + 1];
    let r = data[byte_offset + 2];
    let a = data[byte_offset + 3];

    println!("rect_smoke: center pixel BGRA = ({b}, {g}, {r}, {a})");

    // Red must dominate and blue/green must be near zero.
    assert!(
        r > 200,
        "expected center pixel red channel > 200, got r={r} (BGRA={b},{g},{r},{a})"
    );
    assert!(
        g < 10,
        "expected center pixel green channel < 10, got g={g} (BGRA={b},{g},{r},{a})"
    );
    assert!(
        b < 10,
        "expected center pixel blue channel < 10, got b={b} (BGRA={b},{g},{r},{a})"
    );

    drop(data);
    readback_buf.unmap();

    println!("rect_smoke: OK");
}
