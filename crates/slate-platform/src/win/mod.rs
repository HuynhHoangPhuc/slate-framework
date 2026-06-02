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
mod menu;
mod message_loop;
mod platform;
mod window;

pub use platform::WinPlatform;
pub use window::WinWindow;

use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::{A11yAction, Event, WindowId};

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

/// Custom message carrying a deferred native-menu activation. Posted by the
/// context-menu path (`menu::pop_up_context_menu`) so the selection is
/// dispatched on a fresh pump turn, after the right-click handler that opened
/// the menu has released the dispatch borrow. `wParam` holds the framework
/// `MenuId` (full `u64`; `usize` is 64-bit on the x64 target).
pub(crate) const WM_APP_MENU: u32 = WM_APP + 2;

/// Custom message carrying a deferred screen-reader (Narrator/UIA) action.
/// Posted by [`post_accessibility_action`] so the focus/activation lands on a
/// fresh pump turn after any in-flight handler unwinds — UIA action callbacks
/// can fire mid-stack (and on a non-main thread) while a dispatch borrow is
/// held, exactly like the native-menu deferral. `wParam` encodes the
/// [`A11yAction`] (`0` = Focus, `1` = Activate); `lParam` carries the node's
/// routing id (`ElementId` numeric value, full `u64` on the x64 target).
pub(crate) const WM_APP_A11Y: u32 = WM_APP + 3;

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

// ---------------------------------------------------------------------------
// WindowId → HWND registry (accessibility action posting)
// ---------------------------------------------------------------------------

/// Maps each live window's [`WindowId`] to its raw `HWND` (`isize`) so
/// [`post_accessibility_action`] can target the right window's message queue
/// from any thread. macOS routes a11y actions through the app-global event
/// queue carrying the `WindowId`; on Windows a posted message must name a
/// concrete `HWND`, hence this small registry. Guarded by a `Mutex` because the
/// UIA action handler may run off the main thread.
static WINDOW_HWNDS: Mutex<Vec<(u64, isize)>> = Mutex::new(Vec::new());

/// Record `id → hwnd` so later a11y actions resolve to this window. Called once
/// per window at creation.
pub(crate) fn register_window_hwnd(id: WindowId, hwnd: HWND) {
    if let Ok(mut map) = WINDOW_HWNDS.lock() {
        map.push((id.0, hwnd.0 as isize));
    }
}

/// Drop a window's registry entry at teardown so its id never resolves to a
/// dead `HWND`. Called from `WM_DESTROY`.
pub(crate) fn forget_window_hwnd(id: WindowId) {
    if let Ok(mut map) = WINDOW_HWNDS.lock() {
        map.retain(|(wid, _)| *wid != id.0);
    }
}

fn hwnd_for_window(id: WindowId) -> Option<HWND> {
    let map = WINDOW_HWNDS.lock().ok()?;
    map.iter()
        .find(|(wid, _)| *wid == id.0)
        .map(|(_, raw)| HWND(*raw as *mut _))
}

/// Wire encoding of an [`A11yAction`] for the `WM_APP_A11Y` message `wParam`.
/// Exhaustive within slate-platform so a new variant forces a deliberate code.
fn a11y_action_to_code(action: A11yAction) -> usize {
    match action {
        A11yAction::Focus => 0,
        A11yAction::Activate => 1,
    }
}

/// Decode a `WM_APP_A11Y` `wParam` back into an [`A11yAction`]; `None` for an
/// unknown code. Paired with [`a11y_action_to_code`] so post and dispatch share
/// one source of truth for the wire contract.
pub(crate) fn a11y_action_from_code(code: usize) -> Option<A11yAction> {
    match code {
        0 => Some(A11yAction::Focus),
        1 => Some(A11yAction::Activate),
        _ => None,
    }
}

/// Post a deferred screen-reader action to the targeted window's message queue,
/// to be dispatched as [`Event::AccessibilityAction`] on the next pump turn.
///
/// Thread-safe (the UIA action handler may invoke it off the main thread):
/// `PostMessageW` is safe to call from any thread against a valid `HWND`.
/// Deferral mirrors the native-menu path — Narrator can invoke an action while
/// Slate is mid-stack (a render/handler borrow held), so dispatching the
/// resulting focus/activation inline would hit the re-entrancy guard in
/// [`dispatch_event`] and be dropped. Unknown/dead windows are silently
/// dropped.
pub fn post_accessibility_action(window: WindowId, node: u64, action: A11yAction) {
    let Some(hwnd) = hwnd_for_window(window) else {
        return;
    };
    let code = a11y_action_to_code(action);
    // SAFETY: PostMessageW is thread-safe against a valid HWND; the entry is
    // removed in `WM_DESTROY` so a resolved HWND is live at post time.
    let _ = unsafe { PostMessageW(Some(hwnd), WM_APP_A11Y, WPARAM(code), LPARAM(node as isize)) };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a11y_action_wire_encoding_roundtrips() {
        // post_accessibility_action encodes; the WM_APP_A11Y arm decodes — they
        // must agree, so a drift in one without the other fails here.
        for action in [A11yAction::Focus, A11yAction::Activate] {
            let code = a11y_action_to_code(action);
            assert_eq!(a11y_action_from_code(code), Some(action));
        }
    }

    #[test]
    fn a11y_action_codes_are_distinct_and_stable() {
        assert_eq!(a11y_action_to_code(A11yAction::Focus), 0);
        assert_eq!(a11y_action_to_code(A11yAction::Activate), 1);
    }

    #[test]
    fn unknown_a11y_code_decodes_to_none() {
        assert_eq!(a11y_action_from_code(2), None);
        assert_eq!(a11y_action_from_code(usize::MAX), None);
    }
}
