//! End-to-end validation of the device-lost recovery state machine.
//!
//! Drives a real off-screen DXGI surface through:
//! `NotLost → DetectedLost → CooldownGate → Retrying → Recovered → NotLost`,
//! confirming that `Renderer::force_device_lost` produces a working renderer
//! with an incremented generation on the other side of recovery.
//!
//! Windows-only (force_device_lost is `#[cfg(target_os = "windows")]`).

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::time::Duration;

use slate_framework::test_support::RecoveryHarness;

#[test]
fn force_device_lost_recovers_to_not_lost_with_higher_generation() {
    let _ = env_logger::builder().is_test(true).try_init();

    let harness = RecoveryHarness::new();
    let probe = harness.probe_force_device_lost(Duration::from_secs(5));

    println!("recovery probe result: {probe:#?}");

    assert!(
        !probe.timed_out_before_trigger,
        "force_device_lost never fired — renderer init may have failed (probe: {probe:?})"
    );
    assert!(
        !probe.timed_out_in_recovery,
        "recovery did not complete within timeout (probe: {probe:?})"
    );
    assert!(
        !probe.gave_up,
        "recovery state machine reached GiveUp (probe: {probe:?})"
    );
    assert!(
        !probe.request_quit,
        "AppSignal::RequestQuit emitted during recovery (probe: {probe:?})"
    );
    assert!(
        probe.returned_to_not_lost,
        "recovery did not return to NotLost (probe: {probe:?})"
    );
    assert!(
        probe.final_generation > probe.initial_generation,
        "renderer generation did not increment: {} -> {} (probe: {probe:?})",
        probe.initial_generation,
        probe.final_generation
    );
}
