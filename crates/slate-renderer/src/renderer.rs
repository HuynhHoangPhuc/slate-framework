//! Core [`Renderer`] implementation — wgpu-based GPU rendering.
//!
//! # Lifecycle contract
//!
//! [`Renderer`] **must** be constructed from inside an `Event::Resumed` handler,
//! after the platform run-loop is alive. On macOS, wgpu surface creation requires
//! a running `NSApplication` (CAMetalLayer attachment + `mainScreen` lookups).
//! Constructing before `Platform::run` returns will panic or produce a null surface.

use std::cell::RefCell;
use std::num::NonZeroU32;
use std::rc::Weak;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use slate_platform::Window;
use wgpu::{
    Adapter, Backends, BindGroup, BindGroupDescriptor, BindGroupEntry, BindingResource, Buffer,
    BufferDescriptor, BufferUsages, Color, CommandEncoderDescriptor, Device, DeviceDescriptor,
    ExperimentalFeatures, Features, Instance, InstanceDescriptor, Limits, LoadOp, MemoryHints,
    Operations, Queue, RenderPassColorAttachment, RenderPassDescriptor, RequestAdapterOptions,
    RequestDeviceError, StoreOp, TextureFormat, Trace,
};

use crate::atlas::{AllocId, Atlas, Format};
use crate::glyph_pipeline::GlyphPipeline;
use crate::image_pipeline::ImagePipeline;
use crate::instanced_rect_pipeline::InstancedRectPipeline;
use crate::pipeline_shared::{self, ViewportUniform};
use crate::scene::Scene;
use crate::device_lost_reason;
use crate::observer::RendererObserver;
use crate::shadow_pipeline::ShadowPipeline;
use crate::surface_target::{
    CompositionTarget, ConfigureError, FrameAcquireError,
};

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
    device: Arc<Device>,
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

    // Device-lost state. Atomic so the wgpu `set_device_lost_callback`
    // (Send + 'static, fires from a wgpu-internal thread) can flip the flag.
    // Reads happen on the main thread via `is_device_lost()`.
    device_lost: Arc<AtomicBool>,

    // Observer infrastructure (Phase 1: RendererObserver trait).
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
        let device = Arc::new(device);

        // H1 detection: wgpu fires this callback from a wgpu-internal thread
        // for any device-lost condition, including buffer-creation failures
        // (e.g. 0x887A0005 during cross-monitor drag on hybrid-GPU laptops)
        // that have no Result return path. The atomic flag is read on the
        // main thread by `is_device_lost()` -> `dispatch_redraw` engages the
        // existing recovery state machine on the next frame.
        //
        // Filter `Destroyed` (matches zed/crates/gpui_wgpu/src/wgpu_context.rs:79)
        // so intentional `Renderer` drop in tests does not trigger recovery.
        let device_lost = Arc::new(AtomicBool::new(false));
        device.set_device_lost_callback({
            let device_lost = Arc::clone(&device_lost);
            let device_weak = Arc::downgrade(&device);
            move |reason, message| {
                if reason == wgpu::DeviceLostReason::Destroyed {
                    log::debug!(
                        target: "slate::device_lost",
                        "wgpu device dropped (intentional): {message}"
                    );
                    return;
                }
                let dlr = if let Some(device) = device_weak.upgrade() {
                    device_lost_reason::capture_from_wgpu(reason, message, Some(&device))
                } else {
                    device_lost_reason::capture_from_wgpu_no_device(reason, message)
                };
                device_lost_reason::emit(&dlr);
                log::warn!(
                    target: "slate::device_lost",
                    "wgpu callback telemetry: surface_hr=0x{:08X} removed_reason={:?} adapter_luid={:?}",
                    dlr.surface_hr as u32,
                    dlr.removed_reason_hr.map(|h| format!("0x{:08X}", h as u32)),
                    dlr.adapter_luid
                );
                // Phase-2-reopen trace: confirm callback fires + capture prev-value
                // so we can distinguish first-fire vs re-fire. `swap` is one atomic
                // op; the returned previous value answers H-A vs H-B without a
                // separate load. Thread name included to confirm worker-vs-main.
                let prev = device_lost.swap(true, Ordering::AcqRel);
                let tid = std::thread::current().id();
                let tname = std::thread::current().name().unwrap_or("<unnamed>").to_string();
                log::trace!(
                    target: "slate::device_lost",
                    "wgpu callback fired: reason={:?} prev_flag={} thread={:?}/{}",
                    reason, prev, tid, tname
                );
            }
        });

        let (w, h) = window.physical_size();
        let mut target = build_target(&instance, &adapter, &device, Arc::clone(&window))?;
        target
            .configure(&device, w.max(1), h.max(1))
            .map_err(|e| RendererError::Configure(format!("{:?}", e)))?;
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
            device_lost,
            generation: AtomicU64::new(1),
            observers: RefCell::new(Vec::new()),
        })
    }

    /// Returns true if the GPU device has been lost (e.g., due to driver reset,
    /// monitor topology change, or TDR). Once true, rendering will fail until
    /// the device is recovered (Phase 5).
    pub fn is_device_lost(&self) -> bool {
        self.device_lost.load(Ordering::Acquire)
    }

    /// Explicitly mark the device as lost. Called by app_state when
    /// RendererError::DeviceLost is returned from render operations.
    pub fn mark_device_lost(&self) {
        self.device_lost.store(true, Ordering::Release);
    }

    /// Proactively check device health via GetDeviceRemovedReason.
    ///
    /// Called from WM_DISPLAYCHANGE and WM_DPICHANGED handlers to detect
    /// device loss before the next Present fails. Returns true if the device
    /// is lost (or status indeterminate on Windows).
    pub fn mark_device_potentially_lost(&self) -> bool {
        let reason = device_lost_reason::capture("Renderer::probe", 0, Some(&self.device));
        let is_lost = match reason.removed_reason_hr {
            Some(0) => false,  // S_OK: healthy
            Some(_) => true,   // any non-zero HR: lost
            #[cfg(target_os = "windows")]
            None => true,      // as_hal failed on Windows → assume lost (AD-12)
            #[cfg(not(target_os = "windows"))]
            None => false,     // non-Windows: no DXGI semantics
        };
        if is_lost {
            device_lost_reason::emit(&reason);
            self.device_lost.store(true, Ordering::Release);
        }
        is_lost
    }

    /// Force device-lost state for testing. Calls ID3D12Device5::RemoveDevice()
    /// to trigger a real DXGI_ERROR_DEVICE_REMOVED from the driver.
    ///
    /// # Safety
    ///
    /// This is a destructive operation that renders the device unusable.
    /// Only available with the `test-hooks` feature or in `#[cfg(test)]`.
    #[cfg(all(target_os = "windows", any(test, feature = "test-hooks")))]
    pub fn force_device_lost(&self) {
        use wgpu::hal::api::Dx12;
        use windows::core::Interface;
        use windows::Win32::Graphics::Direct3D12::ID3D12Device5;

        unsafe {
            if let Some(guard) = self.device.as_hal::<Dx12>() {
                let raw_device = guard.raw_device();
                if let Ok(dev5) = raw_device.cast::<ID3D12Device5>() {
                    log::warn!(target: "slate::device_lost",
                        "force_device_lost: calling ID3D12Device5::RemoveDevice");
                    dev5.RemoveDevice();
                }
            }
        }
        self.device_lost.store(true, Ordering::Release);
    }

    /// Fire the device-lost callback logic for testing.
    ///
    /// Mirrors the real `set_device_lost_callback` closure: filters `Destroyed`
    /// (no-op), otherwise captures telemetry, emits tracing event, sets atomic.
    /// Use to validate the Destroyed filter and state-machine engagement paths
    /// without triggering a real wgpu device-lost condition.
    ///
    /// Only available with the `test-hooks` feature or in `#[cfg(test)]`.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn fire_device_lost_callback_for_test(
        &self,
        reason: wgpu::DeviceLostReason,
        message: String,
    ) -> bool {
        if reason == wgpu::DeviceLostReason::Destroyed {
            log::debug!(
                target: "slate::device_lost",
                "fire_device_lost_callback_for_test: filtered Destroyed reason: {message}"
            );
            return false;
        }

        let dlr = device_lost_reason::capture_from_wgpu(reason, message, Some(&self.device));
        device_lost_reason::emit(&dlr);
        log::warn!(
            target: "slate::device_lost",
            "fire_device_lost_callback_for_test: surface_hr=0x{:08X} removed_reason={:?}",
            dlr.surface_hr as u32,
            dlr.removed_reason_hr.map(|h| format!("0x{:08X}", h as u32))
        );

        let prev = self.device_lost.swap(true, Ordering::AcqRel);
        log::trace!(
            target: "slate::device_lost",
            "fire_device_lost_callback_for_test: prev_flag={}", prev
        );
        true
    }

    /// Register an observer to receive device recreation notifications.
    ///
    /// The observer is stored as a weak reference. Dead observers are
    /// automatically pruned during `fire_observers`.
    pub fn register_observer(&self, weak: Weak<dyn RendererObserver>) {
        self.observers.borrow_mut().push(weak);
    }

    /// Returns the current renderer generation (increments on each rebuild).
    ///
    /// Starts at 1; consumers can use 0 to represent "uninitialized".
    pub fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Fire all registered observers with the incremented generation.
    ///
    /// Called by Phase 3's recovery state machine after successful device rebuild.
    /// Dead `Weak` references are pruned in the same pass.
    ///
    /// Uses clone-then-invoke pattern: collects live observers into a temp vec,
    /// drops the RefCell borrow, then invokes callbacks. Prevents panic if an
    /// observer attempts to call register_observer during its callback.
    pub fn fire_observers(&self) {
        let new_gen = self.generation.fetch_add(1, Ordering::AcqRel) + 1;

        // Collect live observers and prune dead ones in a single pass
        let live_observers: Vec<_> = {
            let mut subs = self.observers.borrow_mut();
            let mut live = Vec::with_capacity(subs.len());
            subs.retain(|w| {
                if let Some(strong) = w.upgrade() {
                    live.push(strong);
                    true
                } else {
                    false
                }
            });
            live
        }; // borrow dropped here

        // Invoke callbacks without holding the RefCell borrow
        for observer in &live_observers {
            observer.on_renderer_recreated(new_gen);
        }

        log::debug!(target: "slate::device_lost",
            "fire_observers: generation={}, active_observers={}", new_gen, live_observers.len());
    }

    /// Check if an HRESULT indicates device-lost state. If so, sets the flag,
    /// emits telemetry, and returns true. Called by internal error handlers.
    fn check_hr_for_device_lost(&self, hr: i32) -> bool {
        // Canonical DXGI device-lost codes
        const DXGI_ERROR_DEVICE_REMOVED: i32 = 0x887A0005_u32 as i32;
        const DXGI_ERROR_DEVICE_RESET: i32 = 0x887A0006_u32 as i32;
        const DXGI_ERROR_DEVICE_HUNG: i32 = 0x887A0007_u32 as i32;
        const DXGI_ERROR_DRIVER_INTERNAL_ERROR: i32 = 0x887A0020_u32 as i32;
        const DXGI_ERROR_ACCESS_LOST: i32 = 0x887A0026_u32 as i32;

        if hr == DXGI_ERROR_DEVICE_REMOVED
            || hr == DXGI_ERROR_DEVICE_RESET
            || hr == DXGI_ERROR_DEVICE_HUNG
            || hr == DXGI_ERROR_DRIVER_INTERNAL_ERROR
            || hr == DXGI_ERROR_ACCESS_LOST
        {
            // Phase 2: Capture and emit structured telemetry
            let reason = device_lost_reason::capture("Renderer::check_hr", hr, Some(&self.device));
            device_lost_reason::emit(&reason);
            self.device_lost.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Resize the surface. `new_size` is in physical pixels, matching
    /// the `(u32, u32)` payload of `Event::WindowResized`.
    pub fn resize(&mut self, new_size: (u32, u32)) {
        // Early-return if device is lost - no point attempting resize
        if self.device_lost.load(Ordering::Acquire) {
            log::trace!(target: "slate::resize", "Renderer::resize skipped: device lost");
            // Phase 2.1 pre-emptive fix: nudge the pump so dispatch_redraw runs and
            // the recovery state machine engages even if WM_EXITSIZEMOVE's delegate
            // calls go silent. request_redraw is idempotent (InvalidateRect-backed).
            self._window.request_redraw();
            return;
        }

        let max = self.device.limits().max_texture_dimension_2d;
        let (w, h) = (new_size.0.max(1).min(max), new_size.1.max(1).min(max));
        log::trace!(target: "slate::resize", "Renderer::resize called: {:?} -> {}x{} (target currently: {:?})", new_size, w, h, self.target.size());
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

        // Use actual target size for viewport uniform - if ResizeBuffers failed,
        // target.size() will be the old size, keeping viewport/render-target in sync.
        let (actual_w, actual_h) = self.target.size();
        log::trace!(target: "slate::resize", "After configure: target.size()={}x{} (requested {}x{})", actual_w, actual_h, w, h);
        self.queue.write_buffer(
            &self.viewport_buf,
            0,
            bytemuck::bytes_of(&ViewportUniform {
                size: [actual_w as f32, actual_h as f32],
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

        // Phase B: RECORD — try acquire, with one retry if Outdated.
        let frame = {
            let mut last_outdated = false;
            let mut acquired = None;
            for _attempt in 0..2 {
                match self.target.acquire_frame() {
                    Ok(f) => {
                        acquired = Some(f);
                        break;
                    }
                    Err(FrameAcquireError::Outdated) => {
                        let (w, h) = self._window.physical_size();
                        if let Err(e) = self.target.configure(&self.device, w.max(1), h.max(1)) {
                            match &e {
                                ConfigureError::ResizeBuffersFailed(hr)
                                | ConfigureError::BackBufferFailed(hr) => {
                                    if self.check_hr_for_device_lost(*hr) {
                                        return Err(RenderError::DeviceLost(format!("{:?}", e)));
                                    }
                                }
                            }
                        }
                        last_outdated = true;
                    }
                    Err(
                        FrameAcquireError::Occluded
                        | FrameAcquireError::Minimized
                        | FrameAcquireError::Timeout,
                    ) => {
                        return Ok(());
                    }
                    Err(FrameAcquireError::DeviceLost(reason)) => {
                        self.device_lost.store(true, Ordering::Release);
                        return Err(RenderError::DeviceLost(reason));
                    }
                    Err(other) => return Err(RenderError::AcquireFailed(other.to_string())),
                }
            }
            match acquired {
                Some(f) => f,
                None => {
                    if last_outdated {
                        log::warn!("renderer: surface still Outdated after retry");
                    }
                    return Ok(());
                }
            }
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
                    view,
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

        // Handle present errors - check for device-lost
        if let Err(e) = self.target.present(frame) {
            if self.check_hr_for_device_lost(e.hr()) {
                return Err(RenderError::DeviceLost(format!("present failed: {:?}", e)));
            }
            log::warn!(target: "slate::render", "present failed: {:?}", e);
        }
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

    /// Acquire the next frame, submit a clear-color pass, and present.
    ///
    /// Stub clear-only path — use [`Renderer::render_scene`] for Scene-driven
    /// rendering.
    pub fn render(&mut self) -> Result<(), RenderError> {
        // Try acquire with one retry if Outdated.
        let frame = {
            let mut last_outdated = false;
            let mut acquired = None;
            for _attempt in 0..2 {
                match self.target.acquire_frame() {
                    Ok(f) => {
                        acquired = Some(f);
                        break;
                    }
                    Err(FrameAcquireError::Outdated) => {
                        let (w, h) = self._window.physical_size();
                        if let Err(e) = self.target.configure(&self.device, w.max(1), h.max(1)) {
                            match &e {
                                ConfigureError::ResizeBuffersFailed(hr)
                                | ConfigureError::BackBufferFailed(hr) => {
                                    if self.check_hr_for_device_lost(*hr) {
                                        return Err(RenderError::DeviceLost(format!("{:?}", e)));
                                    }
                                }
                            }
                        }
                        last_outdated = true;
                    }
                    Err(
                        FrameAcquireError::Occluded
                        | FrameAcquireError::Minimized
                        | FrameAcquireError::Timeout,
                    ) => {
                        return Ok(());
                    }
                    Err(FrameAcquireError::DeviceLost(reason)) => {
                        self.device_lost.store(true, Ordering::Release);
                        return Err(RenderError::DeviceLost(reason));
                    }
                    Err(other) => return Err(RenderError::AcquireFailed(other.to_string())),
                }
            }
            match acquired {
                Some(f) => f,
                None => {
                    if last_outdated {
                        log::warn!("renderer: surface still Outdated after retry");
                    }
                    return Ok(());
                }
            }
        };
        self.draw_clear_pass(&frame.view);

        // Handle present errors - check for device-lost
        if let Err(e) = self.target.present(frame) {
            if self.check_hr_for_device_lost(e.hr()) {
                return Err(RenderError::DeviceLost(format!("present failed: {:?}", e)));
            }
            log::warn!(target: "slate::render", "present failed: {:?}", e);
        }
        Ok(())
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
    #[error("raw window handle error: {0}")]
    RawHandle(String),
    #[error("failed to configure surface: {0}")]
    Configure(String),
}

/// Error occurring during [`Renderer::render`].
#[derive(thiserror::Error, Debug)]
pub enum RenderError {
    #[error("frame acquisition timed out")]
    Timeout,
    #[error("window is occluded")]
    Occluded,
    #[error("failed to acquire surface texture: {0}")]
    AcquireFailed(String),
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
