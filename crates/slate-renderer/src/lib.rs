//! GPU renderer for the slate-framework UI framework, built on `wgpu`.
//!
//! The primary entry point is [`Renderer::render_scene`], which takes a
//! [`Scene`] and draws all layers (shadows → rects → glyphs → images per
//! layer) in a single render pass with one `Queue::submit`.
//!
//! # Lifecycle contract
//!
//! [`Renderer`] **must** be constructed from inside an `Event::Resumed` handler,
//! after the platform run-loop is alive. On macOS, wgpu surface creation requires
//! a running `NSApplication` (CAMetalLayer attachment + `mainScreen` lookups).
//! Constructing before `Platform::run` returns will panic or produce a null surface.
//!
//! Use `pollster::block_on` inside the event handler:
//!
//! ```ignore
//! platform.run(|event| {
//!     if let Event::Resumed = event {
//!         let renderer = pollster::block_on(Renderer::new(Arc::clone(&window)))
//!             .expect("failed to init renderer");
//!     }
//! });
//! ```

pub mod atlas;
pub mod color;
pub mod glyph_pipeline;
pub mod image_pipeline;
pub mod instanced_rect_pipeline;
pub mod pipeline_shared;
pub mod scene;
pub mod shadow_pipeline;
pub mod surface_target;

#[cfg(target_os = "macos")]
mod mac_surface;
#[cfg(target_os = "windows")]
mod win_compose;
pub use color::{
    linear_to_srgb_channel, linear_to_srgb_u8, srgb_channel_to_linear, srgb_to_linear,
    srgb_u8_to_linear, srgb_u8_to_linear_premul,
};
pub use glyph_pipeline::{GlyphPipeline, allocate_glyph};
pub use image_pipeline::ImagePipeline;
pub use instanced_rect_pipeline::InstancedRectPipeline;
pub use pipeline_shared::{ViewportUniform, create_unit_quad, viewport_bind_group_layout};
pub use scene::{GlyphInstance, ImageInstance, Layer, RectInstance, Scene, ShadowInstance};
pub use shadow_pipeline::ShadowPipeline;

use std::num::NonZeroU32;
use std::sync::Arc;

use slate_platform::Window;
use wgpu::{
    Adapter, Backends, BindGroup, BindGroupDescriptor, BindGroupEntry, BindingResource, Buffer,
    BufferDescriptor, BufferUsages, Color, CommandEncoderDescriptor, Device,
    DeviceDescriptor, ExperimentalFeatures, Features, Instance, InstanceDescriptor, Limits, LoadOp,
    MemoryHints, Operations, Queue, RenderPassColorAttachment, RenderPassDescriptor,
    RequestAdapterOptions, RequestDeviceError, StoreOp, TextureFormat, Trace,
};

use crate::surface_target::{CompositionTarget, FrameAcquireError};

use crate::atlas::{AllocId, Atlas, Format};

/// wgpu-based GPU renderer.
///
/// Owns the full rendering stack: surface, device, atlases, pipelines, and
/// shared resources (viewport uniform, unit quad). The primary render path is
/// [`Renderer::render_scene`].
pub struct Renderer {
    _instance: Instance,
    _adapter: Adapter,
    device: Device,
    queue: Queue,
    target: Box<dyn CompositionTarget>,
    _window: Arc<dyn Window>,

    // Shared GPU resources (Phase 7).
    viewport_buf: Buffer,
    viewport_bg: BindGroup,
    unit_quad: Buffer,

    // Atlases.
    image_atlas: Atlas,
    glyph_atlas: Atlas,

    // Pipelines.
    rect_pipeline: InstancedRectPipeline,
    shadow_pipeline: ShadowPipeline,
    image_pipeline: ImagePipeline,
    glyph_pipeline: GlyphPipeline,
}

impl Renderer {
    /// Create a renderer for `window`.
    ///
    /// This is `async` because `request_adapter` and `request_device` are async wgpu calls.
    /// Callers **must** invoke this from inside `Event::Resumed` — see crate-level docs.
    ///
    /// # Errors
    ///
    /// Returns [`RendererError`] if no compatible GPU adapter is found, if the wgpu surface
    /// cannot be created, or if the device request fails.
    pub async fn new(window: Arc<dyn Window>) -> Result<Self, RendererError> {
        // Use platform-specific backends:
        // - macOS: Metal
        // - Windows: DX12 (required for DirectComposition swap chain)
        #[cfg(target_os = "macos")]
        let backends = Backends::METAL;
        #[cfg(target_os = "windows")]
        let backends = Backends::DX12;
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let backends = Backends::PRIMARY;

        let instance = Instance::new(InstanceDescriptor {
            backends,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: Default::default(),
            display: None,
        });

        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|_| RendererError::NoAdapter)?;

        log::info!(
            "slate-renderer: GPU adapter selected: {:?}",
            adapter.get_info()
        );

        let (device, queue) = adapter
            .request_device(&DeviceDescriptor {
                label: Some("slate-device"),
                required_features: Features::empty(),
                required_limits: Limits::downlevel_defaults()
                    .using_resolution(adapter.limits())
                    .using_alignment(adapter.limits()),
                memory_hints: MemoryHints::Performance,
                trace: Trace::Off,
                experimental_features: ExperimentalFeatures::disabled(),
            })
            .await?;

        let (w, h) = window.size();
        let mut target = build_target(&instance, &adapter, &device, Arc::clone(&window))?;
        target.configure(&device, w.max(1), h.max(1));
        let format = target.format();

        // --- Shared resources (Phase 7) ---
        let viewport_bgl = pipeline_shared::viewport_bind_group_layout(&device);
        let viewport_buf = device.create_buffer(&BufferDescriptor {
            label: Some("slate-viewport-buf"),
            size: std::mem::size_of::<ViewportUniform>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (target_w, target_h) = target.size();
        queue.write_buffer(
            &viewport_buf,
            0,
            bytemuck::bytes_of(&ViewportUniform {
                size: [target_w as f32, target_h as f32],
                _pad: [0.0; 2],
            }),
        );
        let viewport_bg = device.create_bind_group(&BindGroupDescriptor {
            label: Some("slate-viewport-bg"),
            layout: &viewport_bgl,
            entries: &[BindGroupEntry {
                binding: 0,
                resource: BindingResource::Buffer(viewport_buf.as_entire_buffer_binding()),
            }],
        });
        let unit_quad = pipeline_shared::create_unit_quad(&device);

        // --- Atlases ---
        let image_atlas = Atlas::new(&device, Format::Rgba8UnormSrgb);
        let glyph_atlas = Atlas::new(&device, Format::R8Unorm);

        // --- Pipelines ---
        let rect_pipeline = InstancedRectPipeline::new(&device, format, &viewport_bgl);
        let shadow_pipeline = ShadowPipeline::new(&device, format, &viewport_bgl);
        let image_pipeline = ImagePipeline::new(&device, format, &viewport_bgl, &image_atlas);
        let glyph_pipeline = GlyphPipeline::new(&device, format, &viewport_bgl, &glyph_atlas);

        Ok(Self {
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
            target,
            _window: window,
            viewport_buf,
            viewport_bg,
            unit_quad,
            image_atlas,
            glyph_atlas,
            rect_pipeline,
            shadow_pipeline,
            image_pipeline,
            glyph_pipeline,
        })
    }

    /// Resize the surface. `new_size` is in physical pixels, matching
    /// the `(u32, u32)` payload of `Event::WindowResized`.
    pub fn resize(&mut self, new_size: (u32, u32)) {
        let max = self.device.limits().max_texture_dimension_2d;
        let (w, h) = (new_size.0.max(1).min(max), new_size.1.max(1).min(max));
        if self.target.size() == (w, h) {
            return;
        }
        self.target.configure(&self.device, w, h);
        self.queue.write_buffer(
            &self.viewport_buf,
            0,
            bytemuck::bytes_of(&ViewportUniform {
                size: [w as f32, h as f32],
                _pad: [0.0; 2],
            }),
        );
    }

    /// Tell the composition target whether the window is minimized.
    /// When minimized, present becomes a no-op.
    pub fn set_minimized(&mut self, minimized: bool) {
        self.target.set_minimized(minimized);
    }

    /// Render a scene: all layers in push order, fixed primitive order within
    /// each layer (shadows → rects → glyphs → images).
    ///
    /// Takes `&mut Scene` so it can call `finish()` internally — callers never
    /// need to remember.
    pub fn render_scene(&mut self, scene: &mut Scene) -> Result<(), RenderError> {
        scene.finish();
        self.image_atlas.begin_frame();
        self.glyph_atlas.begin_frame();

        // Phase A: PREPARE — all mutable upload work, no pass alive.
        self.shadow_pipeline
            .prepare(&self.device, &self.queue, &scene.shadows);
        self.rect_pipeline
            .prepare(&self.device, &self.queue, &scene.rects);
        self.image_pipeline
            .prepare(&self.device, &self.queue, &scene.images);
        self.glyph_pipeline
            .prepare(&self.device, &self.queue, &scene.glyphs);

        // Phase B: RECORD — immutable iteration into a single pass.
        let frame = match self.target.acquire_frame() {
            Ok(f) => f,
            Err(FrameAcquireError::Outdated) => {
                let (w, h) = self._window.size();
                self.target.configure(&self.device, w.max(1), h.max(1));
                return Ok(());
            }
            Err(FrameAcquireError::Occluded | FrameAcquireError::Minimized | FrameAcquireError::Timeout) => {
                return Ok(());
            }
            Err(FrameAcquireError::DeviceLost(reason)) => {
                return Err(RenderError::DeviceLost(reason));
            }
            Err(other) => return Err(RenderError::AcquireFailed(other.to_string())),
        };
        let view = &frame.view;
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("slate-frame-encoder"),
            });
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("slate-frame"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color::TRANSPARENT),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None::<NonZeroU32>,
            });

            for layer in &scene.layers {
                self.shadow_pipeline.record(
                    &mut pass,
                    &self.viewport_bg,
                    &self.unit_quad,
                    layer.shadows.clone(),
                );
                self.rect_pipeline.record(
                    &mut pass,
                    &self.viewport_bg,
                    &self.unit_quad,
                    layer.rects.clone(),
                );
                self.glyph_pipeline.record(
                    &mut pass,
                    &self.viewport_bg,
                    &self.unit_quad,
                    layer.glyphs.clone(),
                );
                self.image_pipeline.record(
                    &mut pass,
                    &self.viewport_bg,
                    &self.unit_quad,
                    layer.images.clone(),
                );
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        self.target.present(frame);
        Ok(())
    }

    /// Mutable access to the image atlas (RGBA8) for uploading textures.
    pub fn image_atlas_mut(&mut self) -> &mut Atlas {
        &mut self.image_atlas
    }

    /// Mutable access to the glyph atlas (R8) for uploading glyph bitmaps.
    pub fn glyph_atlas_mut(&mut self) -> &mut Atlas {
        &mut self.glyph_atlas
    }

    /// Upload pixel data to a previously allocated image-atlas slot.
    ///
    /// Convenience wrapper that avoids the borrow-split problem callers hit
    /// when they need `&Queue` (from `self`) and `&mut Atlas` simultaneously.
    pub fn upload_to_image_atlas(&self, alloc_id: AllocId, pixels: &[u8]) {
        self.image_atlas.upload(&self.queue, alloc_id, pixels);
    }

    /// Upload pixel data to a previously allocated glyph-atlas slot.
    pub fn upload_to_glyph_atlas(&self, alloc_id: AllocId, pixels: &[u8]) {
        self.glyph_atlas.upload(&self.queue, alloc_id, pixels);
    }

    /// Access both glyph atlas (mutable) and queue (immutable) together.
    ///
    /// This helper resolves the borrow-checker conflict when you need to call
    /// methods that require `&mut Atlas` and `&Queue` simultaneously.
    pub fn glyph_atlas_and_queue(&mut self) -> (&mut Atlas, &Queue) {
        (&mut self.glyph_atlas, &self.queue)
    }

    /// Acquire the next frame, submit a clear-color pass, and present.
    ///
    /// Stub clear-only path — use [`Renderer::render_scene`] for Scene-driven
    /// rendering.
    pub fn render(&mut self) -> Result<(), RenderError> {
        let frame = match self.target.acquire_frame() {
            Ok(f) => f,
            Err(FrameAcquireError::Outdated) => {
                let (w, h) = self._window.size();
                self.target.configure(&self.device, w.max(1), h.max(1));
                return Ok(());
            }
            Err(FrameAcquireError::Occluded | FrameAcquireError::Minimized | FrameAcquireError::Timeout) => {
                return Ok(());
            }
            Err(FrameAcquireError::DeviceLost(reason)) => {
                return Err(RenderError::DeviceLost(reason));
            }
            Err(other) => return Err(RenderError::AcquireFailed(other.to_string())),
        };
        self.draw_clear_pass(&frame.view);
        self.target.present(frame);
        Ok(())
    }

    // ----- Accessors for caller-driven pipeline construction -----

    /// The logical `wgpu` device.
    pub fn device(&self) -> &Device {
        &self.device
    }

    /// The command queue.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// The texture format the surface is configured with.
    pub fn surface_format(&self) -> TextureFormat {
        self.target.format()
    }

    // ----- Private helpers -----

    /// Encode and submit a clear-color render pass (dark gray).
    fn draw_clear_pass(&self, view: &wgpu::TextureView) {
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("slate-frame-encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("slate-clear-pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color {
                            r: 0.1,
                            g: 0.1,
                            b: 0.15,
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
                multiview_mask: None::<NonZeroU32>,
            });
        }
        self.queue.submit(std::iter::once(encoder.finish()));
    }
}

// ----- Error types -----

/// Error constructing a [`Renderer`].
#[derive(thiserror::Error, Debug)]
pub enum RendererError {
    #[error("no compatible GPU adapter found — check drivers and backend support")]
    NoAdapter,
    #[error("failed to create wgpu surface: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
    #[error("failed to open GPU device: {0}")]
    Device(#[from] RequestDeviceError),
    /// Raw window/display handle could not be obtained from the platform window.
    #[error("raw window handle error: {0}")]
    RawHandle(String),
}

/// Error occurring during [`Renderer::render`].
#[derive(thiserror::Error, Debug)]
pub enum RenderError {
    /// Frame acquisition timed out; caller should retry next tick.
    #[error("frame acquisition timed out")]
    Timeout,
    /// Window is occluded (minimized / behind another window); skip this frame.
    #[error("window is occluded")]
    Occluded,
    /// Surface acquire failed even after reconfiguration.
    #[error("failed to acquire surface texture: {0}")]
    AcquireFailed(String),
    /// Device lost (GPU reset, driver crash, sleep/wake). Caller must rebuild Renderer.
    #[error("device lost: {0}")]
    DeviceLost(String),
}

// ----- Platform-specific target construction -----

#[cfg(target_os = "macos")]
fn build_target(
    instance: &Instance,
    adapter: &Adapter,
    _device: &Device,
    window: Arc<dyn Window>,
) -> Result<Box<dyn CompositionTarget>, RendererError> {
    Ok(Box::new(mac_surface::MacSurface::new(instance, adapter, window)?))
}

#[cfg(target_os = "windows")]
fn build_target(
    _instance: &Instance,
    _adapter: &Adapter,
    device: &Device,
    window: Arc<dyn Window>,
) -> Result<Box<dyn CompositionTarget>, RendererError> {
    Ok(Box::new(win_compose::WinCompose::new(device, window)?))
}
