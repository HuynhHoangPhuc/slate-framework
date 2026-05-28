//! Windows platform backend using `windows-rs` (Win32).
//!
//! # Thread safety
//! All types in this module are **main-thread-only**. `WinWindow` is deliberately
//! not `Send` or `Sync`; Win32 `HWND`s may only be touched on the thread that
//! created them.
//!
//! # Handler lifetime
//! `WinPlatform::run` accepts a `FnMut(Event)` of any lifetime. Internally it
//! erases the lifetime to `'static` for storage in a `thread_local!` `RefCell`.
//! This is sound because `run` **blocks** for the entire duration of the
//! handler's borrow: the `'static`-cast pointer is cleared before `run` returns,
//! so the raw pointer can never outlive the closure.
//!
//! # Arc ownership protocol
//! The user receives `Arc<WinWindow>` (which wraps `Arc<WinWindowInner>`). The OS
//! holds a `*const WinWindowInner` in `GWLP_USERDATA`. At `WM_NCCREATE` we call
//! `Arc::increment_strong_count` to give the OS a logical Arc reference. At
//! `WM_NCDESTROY` (the documented last-ever message for an HWND) we call
//! `Arc::decrement_strong_count` and clear `GWLP_USERDATA`. No `Box::into_raw`
//! of a duplicate state; single source of truth.

mod ime;
mod keymap;
mod message_loop;
mod platform;
mod window;

pub use platform::WinPlatform;
pub use window::WinWindow;

use std::cell::Cell;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::{Event, WindowId};

// ---------------------------------------------------------------------------
// Thread-local event handler storage (lifetime-erasure pattern — mirrors mac.rs)
// ---------------------------------------------------------------------------

type EventHandler = std::cell::RefCell<Option<Box<dyn FnMut(Event) + 'static>>>;

thread_local! {
    pub(crate) static HANDLER: EventHandler = const { std::cell::RefCell::new(None) };
    pub(crate) static IN_SIZE_MOVE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static NEXT_WINDOW_ID: Cell<u64> = const { Cell::new(1) };
}

pub(crate) const SIZE_MOVE_TIMER_ID: usize = 0x5_1A_7E;

/// Timer id reserved by `Window::schedule_redraw_at`. A single id per window
/// so a second `schedule_redraw_at` call REPLACES the in-flight timer rather
/// than stacking — matches the documented "no double-fire" contract on the
/// trait method.
pub(crate) const REDRAW_TIMER_ID: usize = 0x5_1A_7F;

/// Custom message for wake events from background threads.
pub(crate) const WM_APP_WAKE: u32 = WM_APP + 1;

/// Atomic storage for the main window HWND (used for wake from background).
static WAKE_HWND: AtomicIsize = AtomicIsize::new(0);

/// Register a window handle for wake events.
///
/// Called when a window is created. First window wins (CAS).
pub(crate) fn register_wake_hwnd(hwnd: HWND) {
    let _ = WAKE_HWND.compare_exchange(0, hwnd.0 as isize, Ordering::AcqRel, Ordering::Relaxed);
}

/// Clear the wake HWND if it matches the given handle.
///
/// Called when a window is destroyed to prevent posting to a dead HWND.
pub(crate) fn clear_wake_hwnd(hwnd: HWND) {
    let _ = WAKE_HWND.compare_exchange(hwnd.0 as isize, 0, Ordering::AcqRel, Ordering::Relaxed);
}

/// Wake the main run loop from a background thread.
///
/// Thread-safe. Posts WM_APP_WAKE to the registered window.
/// Used by background executors to signal task completion.
///
/// If no window is registered (before first window created or after all
/// windows destroyed), the wake is silently dropped. This is safe but
/// means background tasks completing in those windows won't trigger a
/// redraw — they'll be picked up on the next event.
pub fn wake_run_loop() {
    let hwnd_raw = WAKE_HWND.load(Ordering::Acquire);
    if hwnd_raw != 0 {
        let hwnd = HWND(hwnd_raw as *mut _);
        // SAFETY: PostMessageW is thread-safe when targeting a valid HWND.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_APP_WAKE, WPARAM(0), LPARAM(0)) };
    } else {
        log::trace!(target: "slate::win", "wake_run_loop: no window registered, wake dropped");
    }
}

pub(crate) fn dispatch_event(event: Event) {
    HANDLER.with(|h| {
        // try_borrow_mut to tolerate re-entrant dispatch.
        //
        // `CreateWindowExW` synchronously fires WM_NCCREATE → WM_CREATE →
        // WM_WINDOWPOSCHANGED → WM_SIZE through wndproc on the same thread
        // before returning. If a user handler calls `AppContext::create_window`
        // (the dynamic mid-loop create API), the resulting `CreateWindowExW`
        // re-enters this funnel while the outer handler still holds the
        // borrow. A `borrow_mut` here would panic and abort the process.
        //
        // The dropped events are non-essential for the dynamic create path:
        // `drain_pending_window_creates` calls `init_surfaces` after the
        // outer handler unwinds, which queries the live window for its size
        // and requests its first redraw directly.
        match h.try_borrow_mut() {
            Ok(mut guard) => {
                if let Some(handler) = guard.as_mut() {
                    handler(event);
                }
            }
            Err(_) => {
                log::trace!(
                    target: "slate::win",
                    "dispatch_event: re-entrant call, event dropped: {:?}",
                    event
                );
            }
        }
    });
}

/// Install an event handler directly into the thread-local slot, bypassing the
/// native run loop. Lets a test exercise the `dispatch_event` funnel — the same
/// path every real Win32 message callback uses — without standing up a window.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn install_event_handler_for_test(handler: Box<dyn FnMut(Event) + 'static>) {
    HANDLER.with(|h| *h.borrow_mut() = Some(handler));
}

/// Push an event through the real dispatch funnel from a test.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn dispatch_event_for_test(event: Event) {
    dispatch_event(event);
}

/// Clear the test-installed handler so a later test starts clean.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn clear_event_handler_for_test() {
    HANDLER.with(|h| *h.borrow_mut() = None);
}

pub(crate) fn next_window_id() -> WindowId {
    NEXT_WINDOW_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        WindowId(id)
    })
}

/// Process-wide count of live native windows.
///
/// Incremented by `WinWindow::new` and decremented inside `WM_DESTROY`. The
/// message loop posts `WM_QUIT` only when the destroyed window was the last
/// one — multi-window apps survive single-window closes.
static LIVE_WINDOW_COUNT: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn increment_live_window_count() {
    LIVE_WINDOW_COUNT.fetch_add(1, Ordering::AcqRel);
}

/// Decrement the live-window count and return the value AFTER the decrement.
/// Saturates at zero so a stray double-destroy cannot underflow.
pub(crate) fn decrement_live_window_count() -> usize {
    let mut current = LIVE_WINDOW_COUNT.load(Ordering::Acquire);
    loop {
        if current == 0 {
            return 0;
        }
        match LIVE_WINDOW_COUNT.compare_exchange_weak(
            current,
            current - 1,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return current - 1,
            Err(actual) => current = actual,
        }
    }
}

// ---------------------------------------------------------------------------
// Wide-string helper
// ---------------------------------------------------------------------------

pub(crate) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}
