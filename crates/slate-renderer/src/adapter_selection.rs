//! Windows-only adapter picker: match window's monitor LUID to a DXGI adapter.
//!
//! Hybrid-GPU laptops expose two physical GPUs (iGPU + dGPU) plus a software
//! fallback (Microsoft Basic Render / WARP). Each external display is wired
//! to exactly one adapter. wgpu's default `HighPerformance` preference picks
//! the dGPU unconditionally, so a window on an iGPU-driven internal display
//! ends up cross-adapter composing — which triggers `DXGI_ERROR_DEVICE_REMOVED`
//! (0x887A0005) during cross-monitor drag.
//!
//! This module enumerates the available adapters, filters software ones, and
//! returns the adapter whose LUID matches the window's current monitor. On
//! no-match or probe failure we fall back to the existing `HighPerformance`
//! path so the renderer still constructs successfully.

#[cfg(target_os = "windows")]
use std::sync::Arc;

#[cfg(target_os = "windows")]
use slate_platform::Window;

#[cfg(target_os = "windows")]
use crate::RendererError;

/// Pick the adapter whose DXGI LUID matches the window's current monitor.
///
/// Falls back to wgpu's default `HighPerformance` request if:
/// - the window does not report a monitor LUID (probe failed / minimized),
/// - no enumerated adapter LUID matches,
/// - or the enumeration list contains only software adapters.
#[cfg(target_os = "windows")]
pub async fn pick_adapter_for_window(
    instance: &wgpu::Instance,
    window: &Arc<dyn Window>,
) -> Result<wgpu::Adapter, RendererError> {
    // Sync probe BEFORE the first .await. Window is `?Send`, so the borrow
    // must not be held across an await suspension point.
    let target_luid = window.current_monitor_luid();

    let all_adapters: Vec<_> = instance.enumerate_adapters(wgpu::Backends::DX12).await;
    let total_enumerated = all_adapters.len();
    let adapters: Vec<_> = all_adapters
        .into_iter()
        .filter(|a| !is_software_adapter(a))
        .collect();

    if let Some(target) = target_luid {
        for adapter in &adapters {
            if adapter_luid(adapter) == Some(target) {
                log::info!(
                    target: "slate::adapter",
                    "matched window monitor LUID {:#018x} to adapter {:?}",
                    target,
                    adapter.get_info()
                );
                return Ok(adapter.clone());
            }
        }
        log::warn!(
            target: "slate::adapter",
            "no enumerated adapter matches monitor LUID {:#018x}; falling back to HighPerformance",
            target
        );
    }

    // WARP-only / software-only systems: filter emptied the non-software list
    // but enumeration produced adapters. Allow fallback so request_adapter
    // can return the software adapter instead of failing with NoAdapter.
    let force_fallback = adapters.is_empty() && total_enumerated > 0;
    if force_fallback {
        log::warn!(
            target: "slate::adapter",
            "no hardware adapter available ({} enumerated, all software); enabling fallback",
            total_enumerated
        );
    }

    instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: force_fallback,
        })
        .await
        .map_err(|_| RendererError::NoAdapter)
}

/// Identifies Microsoft Basic Render Driver / WARP / any CPU-backed adapter.
///
/// WARP can present as `DeviceType::Cpu` OR as a discrete-type adapter with
/// vendor id 0x1414 (Microsoft). Belt-and-suspenders name match catches
/// localized strings that might slip past the structured fields.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn is_software_adapter(adapter: &wgpu::Adapter) -> bool {
    is_software_adapter_info(&adapter.get_info())
}

/// Same predicate operating on a raw `AdapterInfo` so it can be unit-tested
/// without constructing a real wgpu adapter.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn is_software_adapter_info(info: &wgpu::AdapterInfo) -> bool {
    if matches!(info.device_type, wgpu::DeviceType::Cpu) {
        return true;
    }
    if info.vendor == 0x1414 {
        return true;
    }
    let name = info.name.to_lowercase();
    name.contains("basic render") || name.contains("warp")
}

/// Extract the DXGI adapter LUID via wgpu-hal Dx12 interop.
///
/// Returns `None` if the adapter is not a Dx12 backend or the DXGI call fails.
#[cfg(target_os = "windows")]
pub(crate) fn adapter_luid(adapter: &wgpu::Adapter) -> Option<u64> {
    use wgpu::hal::api::Dx12;
    // SAFETY: as_hal yields a guard that derefs to wgpu-hal's Adapter; we
    // call read-only DXGI methods on the embedded IDXGIAdapter3 (via Deref)
    // and drop the guard before returning. No aliasing, no mutation.
    unsafe {
        let guard = adapter.as_hal::<Dx12>()?;
        let raw = guard.raw_adapter();
        let desc = raw.GetDesc().ok()?;
        // HighPart is i32 — zero-extend via u32 to avoid sign-extension
        // corrupting the upper 32 bits of the encoded LUID.
        Some(((desc.AdapterLuid.HighPart as u32 as u64) << 32) | (desc.AdapterLuid.LowPart as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::{AdapterInfo, Backend, DeviceType};

    fn info(name: &str, vendor: u32, device_type: DeviceType) -> AdapterInfo {
        AdapterInfo {
            name: name.to_string(),
            vendor,
            device: 0,
            device_pci_bus_id: String::new(),
            device_type,
            driver: String::new(),
            driver_info: String::new(),
            backend: Backend::Dx12,
            subgroup_min_size: 0,
            subgroup_max_size: 0,
            transient_saves_memory: false,
        }
    }

    #[test]
    fn software_adapter_filter_catches_cpu_type() {
        let i = info("AnyName", 0x10DE, DeviceType::Cpu);
        assert!(is_software_adapter_info(&i));
    }

    #[test]
    fn software_adapter_filter_catches_microsoft_vendor() {
        let i = info("Surprise discrete look", 0x1414, DeviceType::DiscreteGpu);
        assert!(is_software_adapter_info(&i));
    }

    #[test]
    fn software_adapter_filter_catches_basic_render_name() {
        let i = info(
            "Microsoft Basic Render Driver",
            0x10DE,
            DeviceType::DiscreteGpu,
        );
        assert!(is_software_adapter_info(&i));
    }

    #[test]
    fn software_adapter_filter_catches_warp_name() {
        let i = info("WARP Software Adapter", 0x10DE, DeviceType::DiscreteGpu);
        assert!(is_software_adapter_info(&i));
    }

    #[test]
    fn software_adapter_filter_allows_real_dgpu() {
        let i = info("NVIDIA GeForce RTX 4090", 0x10DE, DeviceType::DiscreteGpu);
        assert!(!is_software_adapter_info(&i));
    }

    #[test]
    fn software_adapter_filter_allows_real_igpu() {
        let i = info(
            "Intel(R) Iris(R) Xe Graphics",
            0x8086,
            DeviceType::IntegratedGpu,
        );
        assert!(!is_software_adapter_info(&i));
    }
}
