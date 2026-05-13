//! Device-lost telemetry via `GetDeviceRemovedReason`.
//!
//! Captures and emits structured telemetry when a GPU device-lost condition
//! is detected. The telemetry includes the failing API, surface HRESULT,
//! the result of `ID3D12Device::GetDeviceRemovedReason()`, and a timestamp.

use std::time::Instant;

/// Structured telemetry for a device-lost event.
#[derive(Debug, Clone)]
pub struct DeviceLostReason {
    /// Source API that detected the failure (e.g., "Renderer::resize::configure").
    pub source: &'static str,
    /// HRESULT that triggered detection.
    pub surface_hr: i32,
    /// Result of `GetDeviceRemovedReason()`, None if not D3D12.
    pub removed_reason_hr: Option<i32>,
    /// Adapter LUID (for distinguishing TDR vs cross-adapter migration).
    pub adapter_luid: Option<u64>,
    /// Message supplied by the wgpu device-lost callback (None for HR-only paths).
    pub message: Option<String>,
    /// Timestamp when the event was captured.
    pub timestamp: Instant,
}

/// Capture a device-lost reason snapshot.
///
/// `source` identifies the failing API. `hr` is the surface HRESULT (e.g., from
/// `Present` or `configure`). `device` is optional; if provided and this is
/// D3D12, we call `GetDeviceRemovedReason()` for the underlying cause.
pub fn capture(source: &'static str, hr: i32, device: Option<&wgpu::Device>) -> DeviceLostReason {
    let (removed_reason_hr, adapter_luid) = device
        .map(get_removed_reason_and_luid)
        .unwrap_or((None, None));
    DeviceLostReason {
        source,
        surface_hr: hr,
        removed_reason_hr,
        adapter_luid,
        message: None,
        timestamp: Instant::now(),
    }
}

/// Capture a device-lost reason from the wgpu `set_device_lost_callback` call site.
///
/// The wgpu callback is `Send + 'static` and cannot capture `&Device`, so this
/// constructor produces telemetry without `removed_reason_hr` or `adapter_luid`.
/// The wgpu-provided reason variant is mapped onto `surface_hr` so existing
/// HRESULT-based routing remains uniform; `Unknown` uses the `E_FAIL` sentinel.
///
/// Exhaustive match on `wgpu::DeviceLostReason` — adding a wgpu variant must
/// be a compile error to force routing review.
pub fn capture_from_wgpu_no_device(
    reason: wgpu::DeviceLostReason,
    message: String,
) -> DeviceLostReason {
    let surface_hr = match reason {
        wgpu::DeviceLostReason::Unknown => 0x80004005_u32 as i32, // E_FAIL sentinel
        wgpu::DeviceLostReason::Destroyed => 0x80004005_u32 as i32, // filtered upstream; safe fallback
    };
    DeviceLostReason {
        source: "wgpu::device_lost_callback",
        surface_hr,
        removed_reason_hr: None,
        adapter_luid: None,
        message: Some(message),
        timestamp: Instant::now(),
    }
}

/// Emit a device-lost reason as a structured tracing event.
pub fn emit(reason: &DeviceLostReason) {
    tracing::warn!(
        target: "slate::device_lost",
        source = reason.source,
        surface_hr = format!("0x{:08X}", reason.surface_hr as u32).as_str(),
        removed_reason = ?reason.removed_reason_hr.map(|h| format!("0x{:08X}", h as u32)),
        removed_reason_name = removed_reason_name(reason.removed_reason_hr),
        adapter_luid = ?reason.adapter_luid,
        message = ?reason.message,
        "device lost"
    );
}

/// Map a removed_reason HRESULT to a human-readable name.
fn removed_reason_name(hr: Option<i32>) -> &'static str {
    match hr {
        Some(h) => match h as u32 {
            0x887A0005 => "DEVICE_REMOVED",
            0x887A0006 => "DEVICE_HUNG",
            0x887A0007 => "DEVICE_RESET",
            0x887A0020 => "DRIVER_INTERNAL_ERROR",
            0x887A0026 => "ACCESS_LOST",
            0x80004005 => "E_FAIL",
            0 => "S_OK",
            _ => "UNKNOWN",
        },
        None => "n/a",
    }
}

#[cfg(target_os = "windows")]
fn get_removed_reason_and_luid(device: &wgpu::Device) -> (Option<i32>, Option<u64>) {
    use wgpu::hal::api::Dx12;

    unsafe {
        // Guard-style as_hal (wgpu 29.0.3+)
        let Some(guard) = device.as_hal::<Dx12>() else {
            return (None, None);
        };

        let raw_device = guard.raw_device();

        // GetDeviceRemovedReason returns Result<()>:
        // - Ok(()) means S_OK (device healthy)
        // - Err(e) means device lost, e.code() is the HRESULT
        let removed_hr = match raw_device.GetDeviceRemovedReason() {
            Ok(()) => 0, // S_OK - device is healthy
            Err(e) => e.code().0, // Extract HRESULT from error
        };

        // Get adapter LUID for distinguishing TDR from cross-adapter migration
        let luid = get_adapter_luid(raw_device);

        (Some(removed_hr), luid)
    }
}

#[cfg(target_os = "windows")]
fn get_adapter_luid(
    device: &windows::Win32::Graphics::Direct3D12::ID3D12Device,
) -> Option<u64> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;

    unsafe {
        // Query IDXGIDevice from the D3D12 device
        let dxgi_device: Result<IDXGIDevice, _> = device.cast();
        let Ok(dxgi_device) = dxgi_device else {
            return None;
        };

        // Get the adapter - GetDesc now returns the value directly
        let adapter = dxgi_device.GetAdapter().ok()?;
        let desc = adapter.GetDesc().ok()?;

        // Pack LUID (high + low 32 bits)
        let luid = ((desc.AdapterLuid.HighPart as u64) << 32)
            | (desc.AdapterLuid.LowPart as u64);
        Some(luid)
    }
}

#[cfg(not(target_os = "windows"))]
fn get_removed_reason_and_luid(_device: &wgpu::Device) -> (Option<i32>, Option<u64>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_reason_names() {
        assert_eq!(removed_reason_name(Some(0x887A0005_u32 as i32)), "DEVICE_REMOVED");
        assert_eq!(removed_reason_name(Some(0x887A0006_u32 as i32)), "DEVICE_HUNG");
        assert_eq!(removed_reason_name(Some(0)), "S_OK");
        assert_eq!(removed_reason_name(None), "n/a");
    }

    #[test]
    fn capture_without_device() {
        let reason = capture("test::source", 0x887A0005_u32 as i32, None);
        assert_eq!(reason.source, "test::source");
        assert_eq!(reason.surface_hr, 0x887A0005_u32 as i32);
        assert!(reason.removed_reason_hr.is_none());
        assert!(reason.adapter_luid.is_none());
        assert!(reason.message.is_none());
    }

    #[test]
    fn capture_from_wgpu_no_device_unknown() {
        let reason = capture_from_wgpu_no_device(
            wgpu::DeviceLostReason::Unknown,
            "buffer creation failed".into(),
        );
        assert_eq!(reason.source, "wgpu::device_lost_callback");
        assert_eq!(reason.surface_hr, 0x80004005_u32 as i32);
        assert!(reason.removed_reason_hr.is_none());
        assert!(reason.adapter_luid.is_none());
        assert_eq!(reason.message.as_deref(), Some("buffer creation failed"));
    }

    #[test]
    fn e_fail_renders_with_name() {
        assert_eq!(removed_reason_name(Some(0x80004005_u32 as i32)), "E_FAIL");
    }
}
