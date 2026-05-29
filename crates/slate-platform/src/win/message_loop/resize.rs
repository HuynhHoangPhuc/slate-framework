//! Win32 resize, paint, timer, and display-topology handlers.
//!
//! Covers WM_SIZE, WM_TIMER (size-move + scheduled redraw), WM_PAINT,
//! WM_ERASEBKGND, WM_ENTERSIZEMOVE / WM_EXITSIZEMOVE, WM_DISPLAYCHANGE,
//! WM_DPICHANGED, WM_WINDOWPOSCHANGED, WM_GETMINMAXINFO.

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    InvalidateRect, MONITOR_DEFAULTTONULL, MonitorFromWindow, ValidateRect,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, GetClientRect, KillTimer, MINMAXINFO, MSG, PM_REMOVE, PeekMessageW,
    SIZE_MINIMIZED, SWP_NOACTIVATE, SWP_NOZORDER, SetTimer, SetWindowPos, USER_TIMER_MINIMUM,
    WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ENTERSIZEMOVE, WM_ERASEBKGND, WM_EXITSIZEMOVE,
    WM_GETMINMAXINFO, WM_PAINT, WM_SIZE, WM_TIMER, WM_WINDOWPOSCHANGED,
};

use super::super::{IN_SIZE_MOVE, REDRAW_TIMER_ID, SIZE_MOVE_TIMER_ID, dispatch_event};
use super::WinWindowInner;
use crate::{Event, PhysicalSize};

impl WinWindowInner {
    /// Dispatch a resize/paint/timer/topology Win32 message; returns `None`
    /// if `msg` is outside this family.
    pub(super) fn dispatch_resize(
        &self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Option<LRESULT> {
        match msg {
            WM_SIZE => Some(self.on_size(hwnd, wparam, lparam)),
            WM_TIMER if wparam.0 == SIZE_MOVE_TIMER_ID && IN_SIZE_MOVE.with(|f| f.get()) => {
                self.with_delegate(|d| d.on_redraw(self.id));
                dispatch_event(Event::WindowRedrawRequested { window: self.id });
                let _ = unsafe { ValidateRect(Some(hwnd), None) };
                Some(LRESULT(0))
            }
            // One-shot redraw scheduled via Window::schedule_redraw_at.
            // KillTimer immediately so the id is reusable for the next arm and
            // a stuck-set "armed" flag can't outlive the fire.
            WM_TIMER if wparam.0 == REDRAW_TIMER_ID => {
                // SAFETY: hwnd is valid; KillTimer is idempotent.
                let _ = unsafe { KillTimer(Some(hwnd), REDRAW_TIMER_ID) };
                self.redraw_timer_armed.set(false);
                // SAFETY: hwnd is valid; None invalidates the entire client
                // area, matching `Window::request_redraw`. Letting WM_PAINT
                // handle the actual redraw keeps a single redraw code path.
                let _ = unsafe { InvalidateRect(Some(hwnd), None, false) };
                Some(LRESULT(0))
            }
            WM_ERASEBKGND => Some(LRESULT(1)),
            WM_PAINT => {
                self.with_delegate(|d| d.on_redraw(self.id));
                dispatch_event(Event::WindowRedrawRequested { window: self.id });
                let _ = unsafe { ValidateRect(Some(hwnd), None) };
                Some(LRESULT(0))
            }
            WM_ENTERSIZEMOVE => {
                self.in_size_move.set(true);
                IN_SIZE_MOVE.with(|f| f.set(true));
                let id =
                    unsafe { SetTimer(Some(hwnd), SIZE_MOVE_TIMER_ID, USER_TIMER_MINIMUM, None) };
                if id == 0 {
                    log::error!(
                        "SetTimer failed for size-move loop; live-resize rendering disabled this drag"
                    );
                }
                Some(LRESULT(0))
            }
            WM_EXITSIZEMOVE => Some(self.on_exit_size_move(hwnd)),
            // Proactive device health check on monitor topology change.
            WM_DISPLAYCHANGE => {
                if self.in_size_move.get() || IN_SIZE_MOVE.with(|f| f.get()) {
                    log::trace!(target: "slate::win", "WM_DISPLAYCHANGE: in modal loop, deferring probe");
                    self.pending_display_change.set(true);
                } else {
                    log::trace!(target: "slate::win", "WM_DISPLAYCHANGE: probing device health");
                    self.with_delegate(|d| d.on_display_change(self.id));
                }
                Some(LRESULT(0))
            }
            WM_DPICHANGED => Some(self.on_dpi_changed(hwnd, lparam)),
            // Cross-monitor drag without DPI change (same-DPI monitor pair) does
            // not produce WM_DISPLAYCHANGE or WM_DPICHANGED. Compare the
            // window's current HMONITOR against the cached one and request a
            // redraw on change so the framework's per-frame adapter-LUID probe
            // can migrate the renderer. MONITOR_DEFAULTTONULL — we want a real
            // HMONITOR or nothing, never a stale fallback.
            WM_WINDOWPOSCHANGED => Some(self.on_window_pos_changed(hwnd, msg, wparam, lparam)),
            WM_GETMINMAXINFO => Some(self.on_get_min_max_info(lparam)),
            _ => None,
        }
    }

    fn on_size(&self, hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        // Ignore minimize: lparam carries (0, 0) which would stage a
        // zero-dim resize and trigger a swapchain reconfigure to 0×0.
        if wparam.0 == SIZE_MINIMIZED as usize {
            return LRESULT(0);
        }
        // lParam carries physical client size under PMv2.
        let lp = lparam.0 as u32;
        let pw = (lp & 0xFFFF).max(1);
        let ph = ((lp >> 16) & 0xFFFF).max(1);
        // SAFETY: hwnd is valid.
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        let scale = dpi as f64 / 96.0;
        let lw = (pw as f64 / scale).round() as u32;
        let lh = (ph as f64 / scale).round() as u32;
        let in_size_move = IN_SIZE_MOVE.with(|f| f.get());
        log::trace!(target: "slate::win", "WM_SIZE pw={pw} ph={ph} in_size_move={in_size_move}");
        // Sync delegate: skip during size-move (WM_TIMER handles live-resize).
        if !self.in_size_move.get() {
            self.with_delegate(|d| d.on_resize_sync(self.id, PhysicalSize::new(pw, ph)));
        }
        dispatch_event(Event::WindowResized {
            window: self.id,
            logical_size: (lw, lh),
            physical_size: (pw, ph),
            scale_factor: scale,
        });
        if !in_size_move {
            dispatch_event(Event::WindowRedrawRequested { window: self.id });
        }
        LRESULT(0)
    }

    fn on_exit_size_move(&self, hwnd: HWND) -> LRESULT {
        self.in_size_move.set(false);
        IN_SIZE_MOVE.with(|f| f.set(false));
        let _ = unsafe { KillTimer(Some(hwnd), SIZE_MOVE_TIMER_ID) };

        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, Some(hwnd), WM_TIMER, WM_TIMER, PM_REMOVE).as_bool() }
        {
            // discard
        }

        // Fire deferred device probe if display changed during modal loop.
        if self.pending_display_change.get() {
            self.pending_display_change.set(false);
            log::trace!(target: "slate::win", "WM_EXITSIZEMOVE: firing deferred display change probe");
            self.with_delegate(|d| d.on_display_change(self.id));
        }
        // Cross-monitor drag completed inside the modal loop — request a
        // redraw so the framework's per-frame adapter-LUID probe runs
        // and migrates the renderer to the new monitor's adapter if
        // needed. Deliberately separate from `pending_display_change`
        // (different probe semantics: GPU health vs. adapter LUID).
        if self.pending_monitor_change.get() {
            self.pending_monitor_change.set(false);
            log::trace!(target: "slate::win", "WM_EXITSIZEMOVE: draining deferred monitor-change redraw");
            dispatch_event(Event::WindowRedrawRequested { window: self.id });
        }
        log::trace!(target: "slate::win", "WM_EXITSIZEMOVE: post-probe checkpoint reached");

        self.with_delegate(|d| d.on_size_move_end(self.id));
        self.with_delegate(|d| d.on_redraw(self.id));
        dispatch_event(Event::WindowRedrawRequested { window: self.id });
        LRESULT(0)
    }

    fn on_dpi_changed(&self, hwnd: HWND, lparam: LPARAM) -> LRESULT {
        // SAFETY: Win32 guarantees lParam is a valid *const RECT for WM_DPICHANGED.
        let suggested = unsafe { &*(lparam.0 as *const RECT) };
        // SetWindowPos must honour the OS-suggested rect to prevent DPI feedback loops.
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
        // In-size-move guard: suppress resize dispatch during modal loop.
        if !IN_SIZE_MOVE.with(|f| f.get()) {
            // Suggested RECT is frame coords; use GetClientRect for client size.
            let mut rect = RECT::default();
            let _ = unsafe { GetClientRect(hwnd, &mut rect) };
            let pw = (rect.right - rect.left) as u32;
            let ph = (rect.bottom - rect.top) as u32;
            // SAFETY: hwnd is valid.
            let dpi = unsafe { GetDpiForWindow(hwnd) };
            let scale = dpi as f64 / 96.0;
            let lw = (pw as f64 / scale).round() as u32;
            let lh = (ph as f64 / scale).round() as u32;
            // Sync delegate first so framebuffer is ready before observers see resize.
            self.with_delegate(|d| d.on_resize_sync(self.id, PhysicalSize::new(pw, ph)));
            dispatch_event(Event::WindowResized {
                window: self.id,
                logical_size: (lw, lh),
                physical_size: (pw, ph),
                scale_factor: scale,
            });
        }
        // WM_DPICHANGED also probes for device loss (cross-monitor mid-drag).
        if self.in_size_move.get() || IN_SIZE_MOVE.with(|f| f.get()) {
            log::trace!(target: "slate::win", "WM_DPICHANGED: in modal loop, deferring probe");
            self.pending_display_change.set(true);
        } else {
            log::trace!(target: "slate::win", "WM_DPICHANGED: probing device health");
            self.with_delegate(|d| d.on_display_change(self.id));
        }
        LRESULT(0)
    }

    fn on_window_pos_changed(
        &self,
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // SAFETY: hwnd is valid for the message lifetime.
        let hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL) };
        let last = self.last_monitor.get();
        if !hmon.is_invalid() && hmon != last {
            self.last_monitor.set(hmon);
            if self.in_size_move.get() || IN_SIZE_MOVE.with(|f| f.get()) {
                log::trace!(target: "slate::win", "WM_WINDOWPOSCHANGED: monitor changed in modal loop, deferring redraw");
                self.pending_monitor_change.set(true);
            } else {
                log::trace!(target: "slate::win", "WM_WINDOWPOSCHANGED: monitor changed, requesting redraw");
                dispatch_event(Event::WindowRedrawRequested { window: self.id });
            }
        }
        // SAFETY: default proc must run so it generates the follow-up
        // WM_SIZE / WM_MOVE messages.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn on_get_min_max_info(&self, lparam: LPARAM) -> LRESULT {
        if let Some((min_w, min_h)) = self.min_size {
            // SAFETY: lParam is a valid *mut MINMAXINFO for WM_GETMINMAXINFO.
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            info.ptMinTrackSize.x = min_w as i32;
            info.ptMinTrackSize.y = min_h as i32;
        }
        LRESULT(0)
    }
}
