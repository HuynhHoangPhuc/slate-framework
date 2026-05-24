//! Headless wgpu device factory for visual regression tests.
//!
//! Duplicated from slate-renderer/tests/common/mod.rs to avoid cross-crate
//! test dependency; keeping them separate prevents a dev-dependency cycle.

use wgpu::{
    Backends, Device, DeviceDescriptor, ExperimentalFeatures, Features, Instance,
    InstanceDescriptor, Limits, MemoryHints, Queue, RequestAdapterOptions, Trace,
};

/// Acquire a wgpu device + queue with no surface attached.
///
/// Returns `None` if no adapter is available (headless CI on a runner without
/// a software adapter). Tests should treat `None` as "skip" rather than fail.
pub fn make_headless_device() -> Option<(Device, Queue)> {
    pollster::block_on(make_headless_device_async())
}

async fn make_headless_device_async() -> Option<(Device, Queue)> {
    let instance = Instance::new(InstanceDescriptor {
        backends: Backends::PRIMARY,
        flags: wgpu::InstanceFlags::default(),
        memory_budget_thresholds: wgpu::MemoryBudgetThresholds::default(),
        backend_options: Default::default(),
        display: None,
    });

    let adapter = instance
        .request_adapter(&RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
        .ok()?;

    let (device, queue) = adapter
        .request_device(&DeviceDescriptor {
            label: Some("slate-text-headless-test-device"),
            required_features: Features::empty(),
            required_limits: Limits::downlevel_defaults(),
            memory_hints: MemoryHints::Performance,
            trace: Trace::Off,
            experimental_features: ExperimentalFeatures::disabled(),
        })
        .await
        .ok()?;

    Some((device, queue))
}
