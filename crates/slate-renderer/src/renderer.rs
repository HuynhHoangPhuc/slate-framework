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
#[cfg(not(target_os = "windows"))]
use wgpu::RequestAdapterOptions;
use wgpu::{
    Adapter, Backends, BindGroup, BindGroupDescriptor, BindGroupEntry, BindingResource, Buffer,
    BufferDescriptor, BufferUsages, Color, CommandEncoderDescriptor, Device, DeviceDescriptor,
    ExperimentalFeatures, Features, Instance, InstanceDescriptor, Limits, LoadOp, MemoryHints,
    Operations, Queue, RenderPassColorAttachment, RenderPassDescriptor, RequestDeviceError,
    StoreOp, TextureFormat, Trace,
};

use crate::atlas::{AllocId, Atlas, Format};
use crate::device_lost_reason;
use crate::glyph_pipeline::GlyphPipeline;
use crate::image_pipeline::ImagePipeline;
use crate::instanced_rect_pipeline::InstancedRectPipeline;
use crate::layer_ordering::{DefaultPainterOrder, LayerOrdering, ScenePrimitive};
use crate::observer::RendererObserver;
use crate::pipeline_shared::{self, ViewportUniform};
use crate::scene::Scene;
use crate::shadow_pipeline::ShadowPipeline;
use crate::surface_target::{CompositionTarget, ConfigureError, FrameAcquireError};
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
        let wgpu_callback_fired = Arc::new(AtomicBool::new(false));
        device.set_device_lost_callback({
            let device_lost = Arc::clone(&device_lost);
            let wgpu_callback_fired = Arc::clone(&wgpu_callback_fired);
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
                // Signal callback origin to the framework's reason classifier.
                // Set with Release; consumer reads with AcqRel swap.
                wgpu_callback_fired.store(true, Ordering::Release);
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
        let rect_pipeline = InstancedRectPipeline::new(&device, format, &viewport_bgl);
        let shadow_pipeline = ShadowPipeline::new(&device, format, &viewport_bgl);
        let image_pipeline = ImagePipeline::new(&device, format, &viewport_bgl, &image_atlas);
        let glyph_pipeline = GlyphPipeline::new(&device, format, &viewport_bgl, &glyph_atlas);

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

    /// Returns true if the GPU device has been lost (e.g., due to driver reset,
    /// monitor topology change, or TDR). Once true, rendering will fail until
    /// the device is recovered.
    pub fn is_device_lost(&self) -> bool {
        self.device_lost.load(Ordering::Acquire)
    }

    /// DXGI adapter LUID this renderer was constructed against. `None` on
    /// macOS or if LUID extraction failed at construction. Used by the
    /// framework's per-redraw LUID probe to detect when the window has moved
    /// to a monitor served by a different adapter — at which point recovery
    /// rebuilds the renderer on the correct adapter.
    pub fn current_adapter_luid(&self) -> Option<u64> {
        self.adapter_luid
    }

    /// Explicitly mark the device as lost. Called by app_state when
    /// RendererError::DeviceLost is returned from render operations.
    pub fn mark_device_lost(&self) {
        self.device_lost.store(true, Ordering::Release);
    }

    /// Consume the "wgpu callback fired" signal. Returns `true` exactly once
    /// per callback invocation; subsequent calls return `false` until the
    /// callback fires again.
    ///
    /// Used by AppState's reason classifier on the NotLost→DetectedLost edge
    /// and on every redraw during a recovery cycle (upgrade rule). Also
    /// called on Retrying→Recovered transition to discard any late-arriving
    /// signal so it cannot leak into the next cycle's classification.
    pub fn consume_wgpu_callback_fired(&self) -> bool {
        self.wgpu_callback_fired.swap(false, Ordering::AcqRel)
    }

    /// Proactively check device health via GetDeviceRemovedReason.
    ///
    /// Called from WM_DISPLAYCHANGE and WM_DPICHANGED handlers to detect
    /// device loss before the next Present fails. Returns true if the device
    /// is lost (or status indeterminate on Windows).
    pub fn mark_device_potentially_lost(&self) -> bool {
        let reason = device_lost_reason::capture("Renderer::probe", 0, Some(&self.device));
        let is_lost = match reason.removed_reason_hr {
            Some(0) => false, // S_OK: healthy
            Some(_) => true,  // any non-zero HR: lost
            #[cfg(target_os = "windows")]
            None => true, // as_hal failed on Windows → assume lost (AD-12)
            #[cfg(not(target_os = "windows"))]
            None => false, // non-Windows: no DXGI semantics
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
        use windows::Win32::Graphics::Direct3D12::ID3D12Device5;
        use windows::core::Interface;

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
        // Mirror production: signal callback origin so the classifier picks
        // DeviceLossReason::WgpuCallback for `Unknown`-reason test fires.
        self.wgpu_callback_fired.store(true, Ordering::Release);
        log::trace!(
            target: "slate::device_lost",
            "fire_device_lost_callback_for_test: prev_flag={}", prev
        );
        true
    }

    /// Test hook: simulate a LUID-migration-origin device loss without
    /// setting the wgpu-callback atomic. The framework's reason classifier
    /// will pick `DeviceLossReason::LuidMigration` because
    /// `wgpu_callback_fired` stays `false`.
    ///
    /// Only available with the `test-hooks` feature or in `#[cfg(test)]`.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn force_device_lost_luid_migration(&self) {
        self.device_lost.store(true, Ordering::Release);
        // Intentionally do NOT touch wgpu_callback_fired — that is the
        // signal that distinguishes LuidMigration from WgpuCallback.
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
    /// Called by the recovery state machine after successful device rebuild.
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
            // Capture and emit structured telemetry
            let reason = device_lost_reason::capture("Renderer::check_hr", hr, Some(&self.device));
            device_lost_reason::emit(&reason);
            self.device_lost.store(true, Ordering::Release);
            true
        } else {
            false
        }
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

    /// Render a scene: all layers in push order, fixed primitive order within
    /// each layer (shadows → rects → glyphs → images).
    ///
    /// Takes `&mut Scene` so it can call `finish()` internally — callers never
    /// need to remember.
    pub fn render_scene(&mut self, scene: &mut Scene) -> Result<(), RenderError> {
        self.render_scene_with_present_mode(scene, false)
    }

    /// macOS-only: render + present inside AppKit's currently open resize
    /// CATransaction. Use during live resize to land the new framebuffer in
    /// the same transaction as the bounds change. See
    /// [`mac_surface::MacSurface::present_sync`] for the GPU-sync details.
    #[cfg(target_os = "macos")]
    pub fn render_scene_sync(&mut self, scene: &mut Scene) -> Result<(), RenderError> {
        self.render_scene_with_present_mode(scene, true)
    }

    fn render_scene_with_present_mode(
        &mut self,
        scene: &mut Scene,
        sync_present: bool,
    ) -> Result<(), RenderError> {
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

            // Draw order is driven through the `LayerOrdering` seam so the
            // strategy (depth buckets, BoundsTree) can be swapped post-v1
            // without touching this pass-recording code. The default is the v1
            // painter's walk; monomorphized here, so no per-frame vtable.
            DefaultPainterOrder.for_each_draw(scene.layers.len(), |idx, kind| {
                let layer = &scene.layers[idx];
                match kind {
                    ScenePrimitive::Shadow => self.shadow_pipeline.record(
                        &mut pass,
                        &self.viewport_bg,
                        &self.unit_quad,
                        layer.shadows.clone(),
                    ),
                    ScenePrimitive::Rect => self.rect_pipeline.record(
                        &mut pass,
                        &self.viewport_bg,
                        &self.unit_quad,
                        layer.rects.clone(),
                    ),
                    ScenePrimitive::Glyph => self.glyph_pipeline.record(
                        &mut pass,
                        &self.viewport_bg,
                        &self.unit_quad,
                        layer.glyphs.clone(),
                    ),
                    ScenePrimitive::Image => self.image_pipeline.record(
                        &mut pass,
                        &self.viewport_bg,
                        &self.unit_quad,
                        layer.images.clone(),
                    ),
                }
            });
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        // Present — on macOS, route through `present_sync` during live resize
        // so the new framebuffer lands inside AppKit's open CATransaction.
        let present_result = {
            #[cfg(target_os = "macos")]
            {
                if sync_present {
                    self.target.present_sync(frame, &self.device)
                } else {
                    self.target.present(frame)
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = sync_present; // unused off-macOS
                self.target.present(frame)
            }
        };
        if let Err(e) = present_result {
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

    /// Access both atlases (glyph + image) and queue together for paint context.
    pub fn atlases_and_queue(&mut self) -> (&mut Atlas, &mut Atlas, &Queue) {
        (&mut self.glyph_atlas, &mut self.image_atlas, &self.queue)
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
