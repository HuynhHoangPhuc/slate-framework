//! Win32 message loop — wndproc trampoline and WinWindowInner message handling.

use std::sync::Arc;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ValidateRect;
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, DefWindowProcW, GWLP_USERDATA, GetWindowLongPtrW, KillTimer, MINMAXINFO, MSG,
    PM_REMOVE, PeekMessageW, PostQuitMessage, SWP_NOACTIVATE, SWP_NOZORDER, SetTimer,
    SetWindowLongPtrW, SetWindowPos, USER_TIMER_MINIMUM, WM_CLOSE, WM_DESTROY, WM_DPICHANGED,
    WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITSIZEMOVE, WM_GETMINMAXINFO, WM_NCCREATE, WM_NCDESTROY,
    WM_PAINT, WM_SIZE, WM_TIMER, SIZE_MINIMIZED,
};

use crate::{Event, WindowId};
use super::{dispatch_event, IN_SIZE_MOVE, SIZE_MOVE_TIMER_ID, WM_APP_WAKE};

// ---------------------------------------------------------------------------
// WinWindowInner — actual HWND state (Arc'd, OS holds raw ptr via GWLP_USERDATA)
// ---------------------------------------------------------------------------

pub struct WinWindowInner {
    pub(crate) hwnd: HWND,
    pub(crate) hinstance: windows::Win32::Foundation::HINSTANCE,
    pub(crate) id: WindowId,
    pub(crate) min_size: Option<(u32, u32)>,
}

impl WinWindowInner {
    /// Translate a Win32 message into a Slate [`Event`] and dispatch it.
    pub(crate) fn handle_message(&self, hwnd: HWND, msg: u32, _wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_CLOSE => {
                dispatch_event(Event::WindowCloseRequested { window: self.id });
                // SAFETY: default proc is always safe to call.
                unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) }
            }
            WM_DESTROY => {
                dispatch_event(Event::WindowDestroyed { window: self.id });
                // SAFETY: PostQuitMessage is always safe to call from a WM_DESTROY handler.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            WM_SIZE => {
                // Ignore minimize: lparam carries (0, 0) which would stage a
                // zero-dim resize and trigger a swapchain reconfigure to 0×0.
                if _wparam.0 == SIZE_MINIMIZED as usize {
                    return LRESULT(0);
                }
                let lp = lparam.0 as u32;
                let w = (lp & 0xFFFF).max(1);
                let h = ((lp >> 16) & 0xFFFF).max(1);
                let in_size_move = IN_SIZE_MOVE.with(|f| f.get());
                log::trace!(target: "slate::win", "WM_SIZE w={w} h={h} in_size_move={in_size_move}");
                dispatch_event(Event::WindowResized {
                    window: self.id,
                    size: (w, h),
                });
                if !in_size_move {
                    dispatch_event(Event::WindowRedrawRequested { window: self.id });
                }
                LRESULT(0)
            }
            WM_TIMER if _wparam.0 == SIZE_MOVE_TIMER_ID && IN_SIZE_MOVE.with(|f| f.get()) => {
                dispatch_event(Event::WindowRedrawRequested { window: self.id });
                let _ = unsafe { ValidateRect(Some(hwnd), None) };
                LRESULT(0)
            }
            WM_ERASEBKGND => {
                LRESULT(1)
            }
            WM_PAINT => {
                dispatch_event(Event::WindowRedrawRequested { window: self.id });
                let _ = unsafe { ValidateRect(Some(hwnd), None) };
                LRESULT(0)
            }
            WM_ENTERSIZEMOVE => {
                IN_SIZE_MOVE.with(|f| f.set(true));
                let id =
                    unsafe { SetTimer(Some(hwnd), SIZE_MOVE_TIMER_ID, USER_TIMER_MINIMUM, None) };
                if id == 0 {
                    log::error!(
                        "SetTimer failed for size-move loop; live-resize rendering disabled this drag"
                    );
                }
                LRESULT(0)
            }
            WM_EXITSIZEMOVE => {
                IN_SIZE_MOVE.with(|f| f.set(false));
                let _ = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID) };

                let mut msg = MSG::default();
                while unsafe {
                    PeekMessageW(&mut msg, Some(hwnd), WM_TIMER, WM_TIMER, PM_REMOVE).as_bool()
                } {
                    // discard
                }

                dispatch_event(Event::WindowRedrawRequested { window: self.id });
                LRESULT(0)
            }
            WM_DPICHANGED => {
                // SAFETY: Win32 guarantees lParam is a valid *const RECT for WM_DPICHANGED.
                let suggested = unsafe { &*(lparam.0 as *const RECT) };
                let _ = unsafe {
                    SetWindowPos(
                        hwnd,
                        None,
                        suggested.left,
                        suggested.top,
                        suggested.right - suggested.left,
                        suggested.bottom - suggested.top,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    )
                };
                if !IN_SIZE_MOVE.with(|f| f.get()) {
                    let w = (suggested.right - suggested.left) as u32;
                    let h = (suggested.bottom - suggested.top) as u32;
                    dispatch_event(Event::WindowResized {
                        window: self.id,
                        size: (w, h),
                    });
                }
                LRESULT(0)
            }
            WM_GETMINMAXINFO => {
                if let Some((min_w, min_h)) = self.min_size {
                    // SAFETY: lParam is a valid *mut MINMAXINFO for WM_GETMINMAXINFO.
                    let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                    info.ptMinTrackSize.x = min_w as i32;
                    info.ptMinTrackSize.y = min_h as i32;
                }
                LRESULT(0)
            }
            _ if msg == WM_APP_WAKE => {
                dispatch_event(Event::Wake);
                LRESULT(0)
            }
            _ => {
                // SAFETY: default proc is always safe to call.
                unsafe { DefWindowProcW(hwnd, msg, _wparam, lparam) }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// WndProc trampoline — extern "system" callback with catch_unwind + abort (C4)
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
    // C4 fix: catch_unwind boundary — panicking across Win32 dispatch is UB on stable.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: lparam for WM_NCCREATE points to a CREATESTRUCTW as specified
        // by Win32. We immediately store the pointer and increment the Arc count.
        if msg == WM_NCCREATE {
            let cs = lparam.0 as *const CREATESTRUCTW;
            let inner_ptr = unsafe { (*cs).lpCreateParams as *const WinWindowInner };
            // C2 fix: increment strong count — OS now holds a logical Arc reference.
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
            // C1 fix: last message this HWND will ever receive.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            unsafe { Arc::decrement_strong_count(inner_ptr) };
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
