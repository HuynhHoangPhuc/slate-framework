//! Smoke test for `Window::schedule_redraw_at`.
//!
//! Windows-gated because only `WinWindow` exposes the
//! `redraw_timer_armed_for_test` hook today. macOS uses a generation-counter
//! design (no flag to observe); its round-trip is covered by operator testing
//! against the caret-blink driver.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use std::time::{Duration, Instant};

use slate_platform::{DefaultPlatform, DefaultWindow, Platform, Window, WindowOptions};

fn make_offscreen() -> std::sync::Arc<DefaultWindow> {
    let platform = DefaultPlatform::new();
    platform.create_window(WindowOptions {
        title: "schedule_redraw_at-test".into(),
        size: (1, 1),
        min_size: None,
        resizable: false,
        visible: false,
        position: Some((-32000, -32000)),
        ..Default::default()
    })
}

#[test]
fn arming_sets_flag() {
    let win = make_offscreen();
    assert!(
        !win.redraw_timer_armed_for_test(),
        "freshly-created window has no redraw timer"
    );

    win.schedule_redraw_at(Instant::now() + Duration::from_millis(50));
    assert!(
        win.redraw_timer_armed_for_test(),
        "schedule_redraw_at arms the timer flag"
    );
}

#[test]
fn rearming_does_not_panic_and_keeps_flag_set() {
    let win = make_offscreen();
    win.schedule_redraw_at(Instant::now() + Duration::from_millis(50));
    // Replacement: previous timer is killed, new one is set.
    win.schedule_redraw_at(Instant::now() + Duration::from_millis(50));
    assert!(
        win.redraw_timer_armed_for_test(),
        "re-arming keeps the flag set (no leak path observed at the API)"
    );
}

#[test]
fn past_deadline_still_arms() {
    // The trait contract says past deadlines fire on the next message-loop
    // iteration — they must still arm a timer (Win32 USER_TIMER_MINIMUM=10ms).
    let win = make_offscreen();
    win.schedule_redraw_at(Instant::now() - Duration::from_secs(1));
    assert!(
        win.redraw_timer_armed_for_test(),
        "past deadline clamps to USER_TIMER_MINIMUM rather than skipping"
    );
}

#[test]
fn close_with_armed_timer_does_not_panic() {
    // Exercises the WM_DESTROY armed-timer cleanup branch: if we drop the
    // window while a redraw timer is pending, KillTimer must run on the way
    // out so the OS doesn't fire WM_TIMER against a dead HWND.
    let win = make_offscreen();
    win.schedule_redraw_at(Instant::now() + Duration::from_secs(60));
    assert!(win.redraw_timer_armed_for_test());
    win.close();
    // No assertion needed beyond "did not panic"; the cleanup branch is
    // observable only via not crashing in the OS message pump on shutdown.
}
