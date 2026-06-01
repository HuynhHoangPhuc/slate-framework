//! Win32 message loop — wndproc trampoline and per-family dispatch.
//!
//! `WinWindowInner` lives here together with the master `handle_message` entry
//! and the `wnd_proc_trampoline`. Per-family handlers live in sibling modules:
//! `key`, `mouse`, `ime`, `resize`. Each exposes one `dispatch_<family>` method
//! returning `Option<LRESULT>` so this file's match stays a thin router.

mod ime;
mod key;
mod mouse;
mod resize;

use std::cell::{Cell, RefCell};
use std::rc::Weak;
use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dxgi::IDXGIFactory2;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, KillTimer, PostQuitMessage,
    SetWindowLongPtrW, WM_CLOSE, WM_DESTROY, WM_NCCREATE, WM_NCDESTROY,
};

use super::{
    REDRAW_TIMER_ID, WM_APP_MENU, WM_APP_WAKE, clear_wake_hwnd, decrement_live_window_count,
    dispatch_event, menu,
};
use crate::{Event, WindowId, WindowImeDelegate, WindowRenderDelegate};

// ---------------------------------------------------------------------------
// WinWindowInner — actual HWND state (Arc'd, OS holds raw ptr via GWLP_USERDATA)
// ---------------------------------------------------------------------------

pub struct WinWindowInner {
    pub(crate) hwnd: HWND,
    pub(crate) hinstance: windows::Win32::Foundation::HINSTANCE,
    pub(crate) id: WindowId,
    pub(crate) min_size: Option<(u32, u32)>,
    /// Bitmask of currently captured mouse buttons.
    pub(crate) captured_buttons: Cell<u8>,
    /// True if TrackMouseEvent is armed for WM_MOUSELEAVE.
    pub(crate) is_tracking_hover: Cell<bool>,
    /// Render delegate for sync resize/redraw callbacks.
    pub(crate) delegate: RefCell<Option<Weak<dyn WindowRenderDelegate>>>,
    /// IME query delegate for sync OS composition queries during `WM_IME_*` handling.
    pub(crate) ime_delegate: RefCell<Option<Weak<dyn WindowImeDelegate>>>,
    /// `WM_DESTROY` reads this to synthesise a final `Event::ImeDisabled` so
    /// the one-Disabled-per-session contract holds across window teardown.
    pub(crate) composition_active: Cell<bool>,
    /// True during modal size-move loop; WM_SIZE skips delegate (WM_TIMER handles it).
    pub(crate) in_size_move: Cell<bool>,
    /// Deferred device-health probe — set by WM_DISPLAYCHANGE/WM_DPICHANGED
    /// during a modal loop and drained in WM_EXITSIZEMOVE.
    pub(crate) pending_display_change: Cell<bool>,
    /// Fire-once gates so delegate-missing diagnostics don't flood logs (~239×/drag).
    pub(crate) logged_no_delegate: Cell<bool>,
    pub(crate) logged_weak_upgrade_none: Cell<bool>,
    /// Cached DXGI factory for monitor→adapter LUID mapping; created once so
    /// the per-redraw LUID check stays microseconds.
    pub(crate) dxgi_factory: Option<IDXGIFactory2>,
    /// HMONITOR most recently occupied; compared in `WM_WINDOWPOSCHANGED`
    /// to detect cross-monitor moves that bypass DPI/display-change paths.
    pub(crate) last_monitor: Cell<HMONITOR>,
    /// Deferred monitor-change redraw set by WM_WINDOWPOSCHANGED during a
    /// modal loop. Distinct semantics from `pending_display_change`.
    pub(crate) pending_monitor_change: Cell<bool>,
    /// High half of a pending UTF-16 surrogate pair from WM_CHAR; cleared on
    /// WM_KEYUP/WM_SYSKEYUP so orphans don't leak into the next keypress.
    pub(crate) pending_high_surrogate: Cell<Option<u16>>,
    /// One-shot redraw timer armed via `Window::schedule_redraw_at`; the flag
    /// lets a re-arm `KillTimer` first and cleanup in `WM_DESTROY`.
    pub(crate) redraw_timer_armed: Cell<bool>,
    /// Set before our own `ReleaseCapture()` so the following
    /// `WM_CAPTURECHANGED` is treated as voluntary, not theft. Without this
    /// gate every mouse-up would fire `CaptureLost`.
    pub(crate) releasing_capture: Cell<bool>,
}

impl WinWindowInner {
    /// Invoke the render delegate if present. Clone-and-drop pattern ensures
    /// no RefCell borrow is held across the trait method call (re-entrancy safe).
    pub(super) fn with_delegate(&self, f: impl FnOnce(&dyn WindowRenderDelegate)) {
        let weak = self.delegate.borrow().clone();
        if let Some(weak) = weak {
            if let Some(strong) = weak.upgrade() {
                f(&*strong);
            } else if !self.logged_weak_upgrade_none.replace(true) {
                // Surface silent no-op when AppState was already dropped.
                // Fire-once per window to keep diagnostic logs readable.
                log::trace!(target: "slate::win", "with_delegate: weak upgrade returned None (logged once)");
            }
        } else if !self.logged_no_delegate.replace(true) {
            // Delegate slot never installed (e.g. example bypasses AppState).
            // Fire-once per window — otherwise spams ~239×/drag.
            log::trace!(target: "slate::win", "with_delegate: no delegate installed (logged once)");
        }
    }

    /// Translate a Win32 message into a Slate [`Event`] and dispatch it.
    pub(crate) fn handle_message(
        &self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_CLOSE => {
                dispatch_event(Event::WindowCloseRequested { window: self.id });
                // SAFETY: default proc is always safe to call.
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
            WM_DESTROY => {
                // Synthesise ImeDisabled if a composition was still active at
                // teardown, so consumers always see a clean session bracket.
                if self.composition_active.replace(false) {
                    dispatch_event(Event::ImeDisabled { window: self.id });
                }
                // Cancel any in-flight redraw timer so the OS does not raise
                // a stray WM_TIMER against a destroyed window.
                if self.redraw_timer_armed.replace(false) {
                    // SAFETY: hwnd is still valid inside WM_DESTROY.
                    let _ = unsafe { KillTimer(Some(hwnd), REDRAW_TIMER_ID) };
                }
                // Drop this window's menu-bar command map so its ids don't
                // outlive the window.
                menu::forget_window(hwnd);
                dispatch_event(Event::WindowDestroyed { window: self.id });
                // Only end the message pump for the LAST surviving window.
                // Earlier waves wired `PostQuitMessage(0)` unconditionally,
                // which torpedoed every multi-window scenario the moment any
                // single window closed.
                if decrement_live_window_count() == 0 {
                    // SAFETY: PostQuitMessage is always safe to call.
                    unsafe { PostQuitMessage(0) };
                }
                LRESULT(0)
            }
            _ if msg == WM_APP_WAKE => {
                dispatch_event(Event::Wake);
                LRESULT(0)
            }
            _ if msg == WM_APP_MENU => {
                // Deferred context-menu activation re-injected by
                // `menu::pop_up_context_menu`; `wparam` carries the MenuId.
                dispatch_event(Event::MenuActivated {
                    window: self.id,
                    id: wparam.0 as u64,
                });
                LRESULT(0)
            }
            _ => self
                .dispatch_resize(hwnd, msg, wparam, lparam)
                .or_else(|| self.dispatch_mouse(hwnd, msg, wparam, lparam))
                .or_else(|| self.dispatch_key(hwnd, msg, wparam, lparam))
                .or_else(|| self.dispatch_ime(hwnd, msg, wparam, lparam))
                .or_else(|| menu::dispatch_command(self.id, hwnd, msg, wparam, lparam))
                // SAFETY: default proc is always safe to call.
                .unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }),
        }
    }
}

// ---------------------------------------------------------------------------
// WndProc trampoline — extern "system" callback with catch_unwind + abort
// ---------------------------------------------------------------------------

/// # SAFETY
/// This is the Win32 window procedure. Called by the OS on the main thread.
/// All Rust dispatch is wrapped in `catch_unwind`; any panic aborts the process
/// rather than unwinding into Win32 (which is UB on stable Rust).
pub(crate) unsafe extern "system" fn wnd_proc_trampoline(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // catch_unwind boundary — panicking across Win32 dispatch is UB on stable.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: lparam for WM_NCCREATE points to a CREATESTRUCTW as specified
        // by Win32. We immediately store the pointer and increment the Arc count.
        if msg == WM_NCCREATE {
            let cs = lparam.0 as *const CREATESTRUCTW;
            let inner_ptr = unsafe { (*cs).lpCreateParams as *const WinWindowInner };
            // Increment strong count — OS now holds a logical Arc reference.
            unsafe { Arc::increment_strong_count(inner_ptr) };
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, inner_ptr as isize) };
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }

        let inner_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WinWindowInner;
        if inner_ptr.is_null() {
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }

        // SAFETY: GWLP_USERDATA is non-null only between WM_NCCREATE and
        // WM_NCDESTROY (where we clear it). Strong count >= 1 throughout.
        let inner = unsafe { &*inner_ptr };
        let res = inner.handle_message(hwnd, msg, wparam, lparam);

        if msg == WM_NCDESTROY {
            // Last message this HWND will ever receive; release the OS Arc reference.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            unsafe { Arc::decrement_strong_count(inner_ptr) };
            // Clear wake HWND to prevent posting to a dead window.
            clear_wake_hwnd(hwnd);
        }

        res
    }));

    match result {
        Ok(lr) => lr,
        Err(_) => {
            log::error!("slate: Rust panic crossed Win32 wnd_proc; aborting");
            std::process::abort();
        }
    }
}
