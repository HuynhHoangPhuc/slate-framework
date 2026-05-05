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
pub mod rect_pipeline;
pub mod scene;
pub mod shadow_pipeline;
pub use color::{
    linear_to_srgb_channel, linear_to_srgb_u8, srgb_channel_to_linear, srgb_to_linear,
    srgb_u8_to_linear, srgb_u8_to_linear_premul,
};
pub use glyph_pipeline::{GlyphPipeline, allocate_glyph};
pub use image_pipeline::ImagePipeline;
pub use instanced_rect_pipeline::InstancedRectPipeline;
pub use pipeline_shared::{ViewportUniform, create_unit_quad, viewport_bind_group_layout};
pub use rect_pipeline::{RectPipeline, RectUniform};
pub use scene::{GlyphInstance, ImageInstance, Layer, RectInstance, Scene, ShadowInstance};
pub use shadow_pipeline::ShadowPipeline;

use std::num::NonZeroU32;
use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use slate_platform::Window;
use wgpu::{
    Adapter, Backends, BindGroup, BindGroupDescriptor, BindGroupEntry, BindingResource, Buffer,
    BufferDescriptor, BufferUsages, Color, CommandEncoderDescriptor, CurrentSurfaceTexture, Device,
    DeviceDescriptor, ExperimentalFeatures, Features, Instance, InstanceDescriptor, Limits, LoadOp,
    MemoryHints, Operations, PresentMode, Queue, RenderPassColorAttachment, RenderPassDescriptor,
    RequestAdapterOptions, RequestDeviceError, StoreOp, Surface, SurfaceConfiguration,
    SurfaceTargetUnsafe, TextureFormat, TextureUsages, TextureViewDescriptor, Trace,
};

use crate::atlas::{Atlas, Format};

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
    surface: Surface<'static>,
    surface_config: SurfaceConfiguration,
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
        let instance = Instance::new(InstanceDescriptor {
            backends: Backends::PRIMARY, // Metal on macOS, DX12 on Windows
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
            backend_options: Default::default(),
            display: None,
        });

        // Extract raw handles from the Arc<dyn Window>. We store the Arc in Self,
        // ensuring the window — and thus these handles — outlive the Surface<'static>.
        //
        // SAFETY: `window` is held in `Self` for the full lifetime of `surface`.
        // `Surface<'static>` is valid because we guarantee the backing window is
        // kept alive by the `_window` field in the same struct.
        let surface_target = {
            let raw_window = window
                .window_handle()
                .map_err(|e| RendererError::RawHandle(e.to_string()))?
                .as_raw();
            let raw_display = window
                .display_handle()
                .map_err(|e| RendererError::RawHandle(e.to_string()))?
                .as_raw();
            SurfaceTargetUnsafe::RawHandle {
                raw_window_handle: raw_window,
                raw_display_handle: Some(raw_display),
            }
        };
        let surface: Surface<'static> = unsafe { instance.create_surface_unsafe(surface_target)? };

        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
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
                required_limits: Limits::downlevel_defaults(),
                memory_hints: MemoryHints::Performance,
                trace: Trace::Off,
                experimental_features: ExperimentalFeatures::disabled(),
            })
            .await?;

        let caps = surface.get_capabilities(&adapter);

        // Prefer `Bgra8UnormSrgb` so blending happens in linear space and the
        // GPU encodes to sRGB on store. Callers MUST hand the renderer LINEAR
        // color values; convert at the boundary via [`crate::color`] helpers if
        // your inputs come from sRGB sources (CSS hex, design tokens, ...).
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| *f == TextureFormat::Bgra8UnormSrgb)
            .unwrap_or(caps.formats[0]);

        // `Window::size()` returns physical pixels already — no scale_factor multiply.
        let (w, h) = window.size();
        let surface_config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: w.max(1),
            height: h.max(1),
            present_mode: PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &surface_config);

        // --- Shared resources (Phase 7) ---
        let viewport_bgl = pipeline_shared::viewport_bind_group_layout(&device);
        let viewport_buf = device.create_buffer(&BufferDescriptor {
            label: Some("slate-viewport-buf"),
            size: std::mem::size_of::<ViewportUniform>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &viewport_buf,
            0,
            bytemuck::bytes_of(&ViewportUniform {
                size: [surface_config.width as f32, surface_config.height as f32],
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
            surface,
            surface_config,
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
        let (w, h) = (new_size.0.max(1), new_size.1.max(1));
        self.surface_config.width = w;
        self.surface_config.height = h;
        self.surface.configure(&self.device, &self.surface_config);
        self.queue.write_buffer(
            &self.viewport_buf,
            0,
            bytemuck::bytes_of(&ViewportUniform {
                size: [w as f32, h as f32],
                _pad: [0.0; 2],
            }),
        );
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
        let frame = self.acquire_frame()?;
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
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
        frame.present();
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

    /// Acquire the next frame, submit a clear-color pass, and present.
    ///
    /// Stub clear-only path — use [`Renderer::render_scene`] for Scene-driven
    /// rendering.
    pub fn render(&mut self) -> Result<(), RenderError> {
        let frame = self.acquire_frame()?;
        self.draw_clear_pass(&frame);
        frame.present();
        Ok(())
    }

    /// Legacy shim: calls `draw` inside a render pass. Use
    /// [`Renderer::render_scene`] instead.
    #[deprecated(note = "use render_scene(&mut Scene) — removed after Phase 8 demo lands")]
    pub fn render_with<F>(&mut self, mut draw: F) -> Result<(), RenderError>
    where
        F: FnMut(&mut wgpu::RenderPass<'_>),
    {
        let frame = self.acquire_frame()?;
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("slate-frame-encoder"),
            });
        {
            let mut rpass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("slate-draw-pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
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
            draw(&mut rpass);
        }
        self.queue.submit(std::iter::once(encoder.finish()));
        frame.present();
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
        self.surface_config.format
    }

    // ----- Private helpers -----

    /// Acquire the next surface texture, reconfiguring on `Outdated`/`Lost` and
    /// retrying once.
    fn acquire_frame(&mut self) -> Result<wgpu::SurfaceTexture, RenderError> {
        match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) => Ok(frame),
            CurrentSurfaceTexture::Suboptimal(frame) => {
                // Suboptimal is still usable — reconfigure happens on the next resize event.
                Ok(frame)
            }
            CurrentSurfaceTexture::Outdated | CurrentSurfaceTexture::Lost => {
                // Reconfigure and retry once.
                self.surface.configure(&self.device, &self.surface_config);
                match self.surface.get_current_texture() {
                    CurrentSurfaceTexture::Success(frame)
                    | CurrentSurfaceTexture::Suboptimal(frame) => Ok(frame),
                    other => Err(RenderError::AcquireFailed(format!("{other:?}"))),
                }
            }
            CurrentSurfaceTexture::Timeout => Err(RenderError::Timeout),
            CurrentSurfaceTexture::Occluded => Err(RenderError::Occluded),
            CurrentSurfaceTexture::Validation => {
                Err(RenderError::AcquireFailed("validation error".to_owned()))
            }
        }
    }

    /// Encode and submit a clear-color render pass (dark gray).
    fn draw_clear_pass(&self, frame: &wgpu::SurfaceTexture) {
        let view = frame.texture.create_view(&TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("slate-frame-encoder"),
            });
        {
            let _pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("slate-clear-pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
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
            // No-op pass: clear-only. For caller-driven draws use `render_with`.
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
}
