//! Device-lost detection, recovery signaling, and observer registry.
//!
//! Submodule for `Renderer`. Holds:
//!
//! - `install_callback` — wgpu `set_device_lost_callback` plumbing used by
//!   [`Renderer::new`]. Returns the two atomics the renderer stores.
//! - All `impl Renderer` methods that read/write device-lost state or fire
//!   the observer registry.
//!
//! State invariants (mirror the field docs on `Renderer`):
//!
//! - `device_lost` is set on ANY device-lost signal (wgpu callback, present
//!   HRESULT, proactive probe). Reads are main-thread; writes can be from
//!   wgpu-internal worker threads.
//! - `wgpu_callback_fired` is set EXCLUSIVELY by the wgpu callback (and the
//!   equivalent test hook). The reason classifier uses it to distinguish
//!   callback-origin loss from LUID-migration-origin loss.
//! - `generation` starts at 1 so consumers can reserve 0 for "uninitialized".

use std::rc::Weak;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use wgpu::Device;

use crate::device_lost_reason;
use crate::observer::RendererObserver;

use super::Renderer;

/// Install the wgpu `set_device_lost_callback` for `device` and return the
/// two atomics the renderer keeps.
///
/// The closure filters `Destroyed` so intentional drop in tests does not
/// trigger recovery (matches `zed/crates/gpui_wgpu/src/wgpu_context.rs:79`).
pub(super) fn install_callback(device: &Arc<Device>) -> (Arc<AtomicBool>, Arc<AtomicBool>) {
    let device_lost = Arc::new(AtomicBool::new(false));
    let wgpu_callback_fired = Arc::new(AtomicBool::new(false));
    device.set_device_lost_callback({
        let device_lost = Arc::clone(&device_lost);
        let wgpu_callback_fired = Arc::clone(&wgpu_callback_fired);
        let device_weak = Arc::downgrade(device);
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
            // `swap` is one atomic op; the returned previous value answers
            // first-fire vs re-fire without a separate load. Thread name
            // included to confirm worker-vs-main origin.
            let prev = device_lost.swap(true, Ordering::AcqRel);
            // Signal callback origin to the framework's reason classifier.
            // Set with Release; consumer reads with AcqRel swap.
            wgpu_callback_fired.store(true, Ordering::Release);
            let tid = std::thread::current().id();
            let tname = std::thread::current()
                .name()
                .unwrap_or("<unnamed>")
                .to_string();
            log::trace!(
                target: "slate::device_lost",
                "wgpu callback fired: reason={:?} prev_flag={} thread={:?}/{}",
                reason, prev, tid, tname
            );
        }
    });
    (device_lost, wgpu_callback_fired)
}

impl Renderer {
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

    /// Override the adapter LUID this renderer reports. Test-only: lets a test
    /// force an adapter-LUID mismatch against the window's monitor LUID without
    /// a second physical GPU, exercising the cross-adapter migration escalation.
    #[cfg(any(test, feature = "test-hooks"))]
    #[doc(hidden)]
    pub fn set_adapter_luid_for_test(&mut self, luid: Option<u64>) {
        self.adapter_luid = luid;
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
            None => true, // as_hal failed on Windows → assume lost
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
    #[doc(hidden)]
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
    #[doc(hidden)]
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
    #[doc(hidden)]
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

    /// Return `Err(RenderError::DeviceLost)` if the device-lost flag is already
    /// set, otherwise `Ok(())`. A cheap atomic load used to bail out of the
    /// per-frame render path the moment a device-lost signal (wgpu callback,
    /// present HRESULT, or proactive probe) is observed — before issuing any
    /// further queue uploads or acquiring a frame on a removed device.
    pub(super) fn device_lost_error_if_set(
        &self,
        context: &'static str,
    ) -> Result<(), super::RenderError> {
        if self.is_device_lost() {
            log::warn!(target: "slate::device_lost", "aborting GPU work — device lost ({context})");
            Err(super::RenderError::DeviceLost(context.to_owned()))
        } else {
            Ok(())
        }
    }

    /// Nudge the platform pump so `dispatch_redraw` re-enters and drives the
    /// recovery state machine. Idempotent (InvalidateRect-backed on Windows).
    pub(super) fn request_recovery_redraw(&self) {
        self._window.request_redraw();
    }

    /// Check if an HRESULT indicates device-lost state. If so, sets the flag,
    /// emits telemetry, and returns true. Called by internal error handlers.
    pub(super) fn check_hr_for_device_lost(&self, hr: i32) -> bool {
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
}
