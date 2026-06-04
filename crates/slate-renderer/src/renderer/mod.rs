//! Core [`Renderer`] implementation — wgpu-based GPU rendering.
//!
//! # Lifecycle contract
//!
//! [`Renderer`] **must** be constructed from inside an `Event::Resumed` handler,
//! after the platform run-loop is alive. On macOS, wgpu surface creation requires
//! a running `NSApplication` (CAMetalLayer attachment + `mainScreen` lookups).
//! Constructing before `Platform::run` returns will panic or produce a null surface.
//!
//! # Module layout
//!
//! - This file owns the [`Renderer`] struct, ctor, surface lifecycle (resize /
//!   minimize), atlas accessors, and error types.
//! - `device_lost` holds all device-lost state, the wgpu callback installer,
//!   and the observer registry.
//! - `submit` holds the per-frame render path.
//! - `pipeline_*` submodules wrap per-pipeline construction so the ctor stays
//!   linear and pipeline init order is the single source of truth here.

mod device_lost;
mod pipeline_image;
mod pipeline_quad;
mod pipeline_shadow;
mod pipeline_text;
mod submit;

use std::cell::RefCell;
use std::rc::Weak;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slate_platform::Window;
#[cfg(not(target_os = "windows"))]
use wgpu::RequestAdapterOptions;
use wgpu::{
    Adapter, Backends, BindGroup, BindGroupDescriptor, BindGroupEntry, BindingResource, Buffer,
    BufferDescriptor, BufferUsages, Device, DeviceDescriptor, ExperimentalFeatures, Features,
    Instance, InstanceDescriptor, Limits, MemoryHints, Queue, RequestDeviceError, TextureFormat,
    Trace,
};

use crate::atlas::{AllocId, Atlas, Format};
use crate::glyph_pipeline::GlyphPipeline;
use crate::image_pipeline::ImagePipeline;
use crate::instanced_rect_pipeline::InstancedRectPipeline;
use crate::observer::RendererObserver;
use crate::pipeline_shared::{self, ViewportUniform};
use crate::shadow_pipeline::ShadowPipeline;
use crate::surface_target::{CompositionTarget, ConfigureError};
use crate::units::Lpx;

#[cfg(target_os = "macos")]
use crate::mac_surface;
#[cfg(target_os = "windows")]
use crate::win_compose;

/// wgpu-based GPU renderer.
///
/// Owns the full rendering stack: surface, device, atlases, pipelines, and
/// shared resources (viewport uniform, unit quad). The primary render path is
/// [`Renderer::render_scene`].
pub struct Renderer {
    _instance: Instance,
    _adapter: Adapter,
    /// DXGI adapter LUID captured at construction. `None` on macOS, or on
    /// Windows if the LUID extraction failed (logged at construction).
    ///
    /// Invariant: this value is captured from the wgpu adapter the renderer
    /// was built on. Because `Renderer` is fully rebuilt on every recovery
    /// (full `Renderer::new` runs, observers re-fire), this stays in lock-step
    /// with the live device. **If partial recovery is ever added that re-uses
    /// the wgpu `Device`, this MUST be refreshed at the same time.**
    adapter_luid: Option<u64>,
    device: Arc<Device>,
    queue: Queue,
    // Concrete on each platform so macOS-only sync-present can reach
    // `MacSurface::present_sync` without polluting the trait or any-downcast
    // scaffolding. Both impls still satisfy `CompositionTarget`, so the
    // trait-style call sites (`self.target.configure(...)`, etc.) work via
    // auto-deref.
    #[cfg(target_os = "macos")]
    target: Box<mac_surface::MacSurface>,
    #[cfg(target_os = "windows")]
    target: Box<dyn CompositionTarget>,
    _window: Arc<dyn Window>,

    // Shared GPU resources.
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

    // Device-lost state. Atomic so the wgpu `set_device_lost_callback`
    // (Send + 'static, fires from a wgpu-internal thread) can flip the flag.
    // Reads happen on the main thread via `is_device_lost()`.
    device_lost: Arc<AtomicBool>,

    // Wgpu-callback origin signal. Set exclusively from inside the wgpu
    // `set_device_lost_callback` closure (and the equivalent test hook).
    // AppState consumes it via `consume_wgpu_callback_fired` on the
    // NotLost→DetectedLost edge and on every subsequent redraw during a
    // recovery cycle to apply the "upgrade rule" (a wgpu-callback that
    // arrives after a LuidMigration cycle upgrades the cycle's reason).
    // Lifecycle: fire-and-be-consumed-once-per-cycle. Cleared on
    // Retrying→Recovered transition to prevent leakage across cycles.
    wgpu_callback_fired: Arc<AtomicBool>,

    // Observer infrastructure (RendererObserver trait).
    // Generation starts at 1 so consumers can reserve 0 for "uninitialized".
    generation: AtomicU64,
    observers: RefCell<Vec<Weak<dyn RendererObserver>>>,
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
        // Platform backends: macOS=Metal, Windows=DX12 (required for
        // DirectComposition swap chain), other=PRIMARY.
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

        #[cfg(target_os = "windows")]
        let adapter = crate::adapter_selection::pick_adapter_for_window(&instance, &window).await?;
        #[cfg(not(target_os = "windows"))]
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

        #[cfg(target_os = "windows")]
        let adapter_luid = crate::adapter_selection::adapter_luid(&adapter);
        #[cfg(not(target_os = "windows"))]
        let adapter_luid: Option<u64> = None;

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
        let device = Arc::new(device);

        // H1 detection: wgpu fires the device-lost callback from a wgpu-internal
        // thread for any device-lost condition, including buffer-creation
        // failures (e.g. 0x887A0005 during cross-monitor drag on hybrid-GPU
        // laptops) that have no Result return path. The atomic flag is read on
        // the main thread by `is_device_lost()` → `dispatch_redraw` engages the
        // existing recovery state machine on the next frame.
        let (device_lost, wgpu_callback_fired) = device_lost::install_callback(&device);

        let (w, h) = window.physical_size();
        let mut target = build_target(&instance, &adapter, &device, Arc::clone(&window))?;
        target
            .configure(&device, w.max(1), h.max(1))
            .map_err(|e| RendererError::Configure(format!("{:?}", e)))?;
        let format = target.format();

        // --- Shared resources ---
        // Built once here and passed by reference into every instanced
        // pipeline below; no pipeline owns its own copy of the viewport BGL or
        // the unit quad. This single-construction sharing is the invariant the
        // `shared_unit_quad` test guards.
        let viewport_bgl = pipeline_shared::viewport_bind_group_layout(&device);
        let viewport_buf = device.create_buffer(&BufferDescriptor {
            label: Some("slate-viewport-buf"),
            size: std::mem::size_of::<ViewportUniform>() as u64,
            usage: BufferUsages::UNIFORM | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Viewport uniform is in *logical* pixels (lpx). The shader maps
        // `pixel_pos / viewport.size`, which is scale-invariant — feeding
        // both numerator and denominator in lpx yields the same NDC as the
        // old physical/physical pair. Surface configuration stays physical.
        let (logical_w, logical_h) = window.logical_size();
        queue.write_buffer(
            &viewport_buf,
            0,
            bytemuck::bytes_of(&ViewportUniform {
                size: [Lpx(logical_w as f32), Lpx(logical_h as f32)],
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
        // Init order matters: Metal/D3D pipeline-state caches and root-signature
        // reuse are sensitive to creation order. Keep this sequence stable.
        let rect_pipeline = pipeline_quad::init(&device, format, &viewport_bgl);
        let shadow_pipeline = pipeline_shadow::init(&device, format, &viewport_bgl);
        let image_pipeline = pipeline_image::init(&device, format, &viewport_bgl, &image_atlas);
        let glyph_pipeline = pipeline_text::init(&device, format, &viewport_bgl, &glyph_atlas);

        Ok(Self {
            _instance: instance,
            _adapter: adapter,
            adapter_luid,
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
            device_lost,
            wgpu_callback_fired,
            generation: AtomicU64::new(1),
            observers: RefCell::new(Vec::new()),
        })
    }

    /// Resize the surface.
    ///
    /// `physical` is the drawable size in physical pixels (used for wgpu
    /// surface configuration). `logical` is the same drawable in logical
    /// pixels (lpx) — written to the viewport uniform. Both are carried by
    /// `Event::WindowResized`; platform callbacks already have both in scope.
    pub fn resize(&mut self, physical: (u32, u32), logical: (u32, u32)) {
        // Early-return if device is lost - no point attempting resize
        if self.device_lost.load(Ordering::Acquire) {
            log::trace!(target: "slate::resize", "Renderer::resize skipped: device lost");
            // Nudge the pump so dispatch_redraw runs and the recovery state machine
            // engages even if WM_EXITSIZEMOVE's delegate calls go silent.
            // request_redraw is idempotent (InvalidateRect-backed).
            self._window.request_redraw();
            return;
        }

        let max = self.device.limits().max_texture_dimension_2d;
        let (w, h) = (physical.0.max(1).min(max), physical.1.max(1).min(max));
        log::trace!(target: "slate::resize", "Renderer::resize called: physical={:?} logical={:?} -> {}x{} (target currently: {:?})", physical, logical, w, h, self.target.size());
        if self.target.size() == (w, h) {
            log::trace!(target: "slate::resize", "Renderer::resize: no change needed");
            return;
        }

        // Handle configure errors - check for device-lost
        if let Err(e) = self.target.configure(&self.device, w, h) {
            match &e {
                ConfigureError::ResizeBuffersFailed(hr) | ConfigureError::BackBufferFailed(hr) => {
                    self.check_hr_for_device_lost(*hr);
                }
            }
            log::warn!(target: "slate::resize", "configure failed: {:?}", e);
            // Continue with old size - viewport uniform will match target.size()
        }

        // Viewport uniform is in logical pixels (lpx). Surface configure stays
        // physical; the shader's `pos / size` NDC math is scale-invariant.
        let (logical_w, logical_h) = logical;
        log::trace!(target: "slate::resize", "After configure: target.size()={:?} viewport-lpx=({}, {})", self.target.size(), logical_w, logical_h);
        self.queue.write_buffer(
            &self.viewport_buf,
            0,
            bytemuck::bytes_of(&ViewportUniform {
                size: [Lpx(logical_w as f32), Lpx(logical_h as f32)],
                _pad: [0.0; 2],
            }),
        );
    }

    /// Tell the composition target whether the window is minimized.
    /// When minimized, present becomes a no-op.
    pub fn set_minimized(&mut self, minimized: bool) {
        self.target.set_minimized(minimized);
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
    pub fn upload_to_image_atlas(&self, alloc_id: AllocId, pixels: &[u8]) {
        self.image_atlas.upload(&self.queue, alloc_id, pixels);
    }

    /// Upload pixel data to a previously allocated glyph-atlas slot.
    pub fn upload_to_glyph_atlas(&self, alloc_id: AllocId, pixels: &[u8]) {
        self.glyph_atlas.upload(&self.queue, alloc_id, pixels);
    }

    /// Access both glyph atlas (mutable) and queue (immutable) together.
    pub fn glyph_atlas_and_queue(&mut self) -> (&mut Atlas, &Queue) {
        (&mut self.glyph_atlas, &self.queue)
    }

    /// Access both atlases (glyph + image) and queue together for paint context.
    pub fn atlases_and_queue(&mut self) -> (&mut Atlas, &mut Atlas, &Queue) {
        (&mut self.glyph_atlas, &mut self.image_atlas, &self.queue)
    }

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

    /// Physical size the composition target is currently configured at.
    /// Test-only observable for the resize-coalescing invariant.
    #[cfg(feature = "test-hooks")]
    pub fn surface_size(&self) -> (u32, u32) {
        self.target.size()
    }
}

// ----- Error types -----

/// Error constructing a [`Renderer`].
#[derive(thiserror::Error, Debug)]
pub enum RendererError {
    /// No GPU adapter that satisfies the backend / surface requirements was found.
    #[error("no compatible GPU adapter found — check drivers and backend support")]
    NoAdapter,
    /// Underlying `wgpu::Instance::create_surface` returned an error.
    #[error("failed to create wgpu surface: {0}")]
    Surface(#[from] wgpu::CreateSurfaceError),
    /// `Adapter::request_device` rejected the device descriptor.
    #[error("failed to open GPU device: {0}")]
    Device(#[from] RequestDeviceError),
    /// Failed to obtain a raw window handle from the platform `Window`.
    #[error("raw window handle error: {0}")]
    RawHandle(String),
    /// `Surface::configure` failed for the initial swap-chain configuration.
    #[error("failed to configure surface: {0}")]
    Configure(String),
}

/// Error occurring during [`Renderer::render`].
#[derive(thiserror::Error, Debug)]
pub enum RenderError {
    /// Frame acquisition timed out — the OS did not return a swap-chain image in time.
    #[error("frame acquisition timed out")]
    Timeout,
    /// Window is occluded; the frame was dropped this tick.
    #[error("window is occluded")]
    Occluded,
    /// Swap-chain image acquisition failed for an unspecified reason.
    #[error("failed to acquire surface texture: {0}")]
    AcquireFailed(String),
    /// GPU device was lost (driver reset, TDR, adapter change) — caller must rebuild.
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
) -> Result<Box<mac_surface::MacSurface>, RendererError> {
    Ok(Box::new(mac_surface::MacSurface::new(
        instance, adapter, window,
    )?))
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
