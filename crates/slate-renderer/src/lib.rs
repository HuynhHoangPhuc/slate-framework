//! GPU renderer for the slate-framework UI framework, built on `wgpu`.
//!
//! Phase 4: surface bring-up + clear-color render pass.
//! Phase 5 will replace the stub render pass with the SDF rounded-rect shader.
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

use std::num::NonZeroU32;
use std::sync::Arc;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use slate_platform::Window;
use wgpu::{
    Adapter, Backends, Color, CommandEncoderDescriptor, CurrentSurfaceTexture, Device,
    DeviceDescriptor, ExperimentalFeatures, Features, Instance, InstanceDescriptor, Limits,
    LoadOp, MemoryHints, Operations, PresentMode, Queue, RenderPassColorAttachment,
    RenderPassDescriptor, RequestAdapterOptions, RequestDeviceError, StoreOp, Surface,
    SurfaceConfiguration, SurfaceTargetUnsafe, TextureFormat, TextureUsages,
    TextureViewDescriptor, Trace,
};

/// wgpu-based GPU renderer.
///
/// Owns `Instance`, `Adapter`, `Device`, `Queue`, `Surface`, and `SurfaceConfiguration`.
/// Also holds the `Arc<dyn Window>` to guarantee the surface stays valid as long as
/// the renderer is alive (wgpu surfaces borrow the window's raw handle).
pub struct Renderer {
    // Instance must be kept alive for adapter/surface lifetimes.
    _instance: Instance,
    // Adapter kept for potential future introspection / Phase 5 pipeline creation.
    _adapter: Adapter,
    device: Device,
    queue: Queue,
    surface: Surface<'static>,
    surface_config: SurfaceConfiguration,
    // Keep window alive so the raw handle underlying the surface is never freed.
    _window: Arc<dyn Window>,
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
        let surface: Surface<'static> =
            unsafe { instance.create_surface_unsafe(surface_target)? };

        let adapter = instance
            .request_adapter(&RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|_| RendererError::NoAdapter)?;

        log::info!("slate-renderer: GPU adapter selected: {:?}", adapter.get_info());

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

        // Prefer Bgra8UnormSrgb for consistent gamma on both macOS and Windows.
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

        Ok(Self {
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
            surface,
            surface_config,
            _window: window,
        })
    }

    /// Resize the surface. `new_size` is in physical pixels, matching
    /// the `(u32, u32)` payload of `Event::WindowResized`.
    pub fn resize(&mut self, new_size: (u32, u32)) {
        let (w, h) = (new_size.0.max(1), new_size.1.max(1));
        self.surface_config.width = w;
        self.surface_config.height = h;
        self.surface.configure(&self.device, &self.surface_config);
    }

    /// Acquire the next frame, submit a clear-color pass, and present.
    ///
    /// On `Outdated` or `Lost`, the surface is reconfigured and the frame is
    /// retried once. If the retry also fails, the error is propagated.
    ///
    /// Phase 5 will replace the stub clear pass with the SDF rect shader.
    pub fn render(&mut self) -> Result<(), RenderError> {
        let frame = self.acquire_frame()?;
        self.draw_clear_pass(&frame);
        frame.present();
        Ok(())
    }

    /// Like [`render`], but calls `draw` inside the render pass so callers can
    /// issue additional draw commands (e.g. `RectPipeline::record`) before submit.
    ///
    /// The clear color and acquire/retry logic are identical to [`render`].
    pub fn render_with<F>(&mut self, mut draw: F) -> Result<(), RenderError>
    where
        F: FnMut(&mut wgpu::RenderPass<'_>),
    {
        let frame = self.acquire_frame()?;
        let view = frame
            .texture
            .create_view(&TextureViewDescriptor::default());
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

    // ----- Accessors for Phase 5 pipeline construction -----

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
        let view = frame
            .texture
            .create_view(&TextureViewDescriptor::default());
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
            // Phase 5 draw calls go here.
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
