//! WinWindow — native Win32 window handle.

use std::cell::{Cell, RefCell};
use std::num::NonZeroIsize;
use std::rc::Weak;
use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
};
use windows::Win32::Foundation::RECT;
use windows::Win32::Foundation::{HINSTANCE, HWND};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIFactory2,
};
use windows::Win32::Graphics::Gdi::{
    HMONITOR, InvalidateRect, MONITOR_DEFAULTTONEAREST, MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DestroyWindow, GetClientRect, GetWindowPlacement, KillTimer,
    SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED, SetForegroundWindow, SetTimer, SetWindowTextW, ShowWindow,
    USER_TIMER_MINIMUM, WINDOWPLACEMENT, WS_DISABLED, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW,
    WS_OVERLAPPEDWINDOW, WS_POPUP, WS_VISIBLE,
};
use windows::core::PCWSTR;

use super::message_loop::WinWindowInner;
use super::platform::CLASS_NAME;
use super::{
    REDRAW_TIMER_ID, increment_live_window_count, next_window_id, register_wake_hwnd,
    register_window_hwnd, to_wide,
};
use crate::{
    PlatformMenu, Window, WindowA11yDelegate, WindowId, WindowImeDelegate, WindowOptions,
    WindowPlacement, WindowRenderDelegate,
};

// ---------------------------------------------------------------------------
// WinWindow — public window handle (thin Arc wrapper)
// ---------------------------------------------------------------------------

/// A native Win32 window.
///
/// Not `Send` or `Sync`; must be used exclusively on the main OS thread.
pub struct WinWindow {
    pub(crate) inner: Arc<WinWindowInner>,
}

impl WinWindow {
    pub(crate) fn new(opts: &WindowOptions, hinstance: HINSTANCE) -> Arc<Self> {
        let id = next_window_id();
        // Track the live-window count so `WM_DESTROY` only posts `WM_QUIT` for
        // the last surviving native window. Multi-window apps need this to
        // outlive a single window close.
        increment_live_window_count();
        let title_w = to_wide(&opts.title);

        // Cache a DXGI factory up-front so per-redraw LUID probes don't
        // re-create one each call. None on failure → probe returns None →
        // adapter selection falls back to HighPerformance.
        // SAFETY: CreateDXGIFactory2 is a pure COM constructor; no aliasing.
        let dxgi_factory: Option<IDXGIFactory2> =
            unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }.ok();
        if dxgi_factory.is_none() {
            log::warn!(
                target: "slate::win",
                "CreateDXGIFactory2 failed; monitor-LUID probe will return None"
            );
        }

        #[allow(clippy::arc_with_non_send_sync)]
        let inner = Arc::new(WinWindowInner {
            hwnd: HWND(std::ptr::null_mut()),
            hinstance,
            id,
            min_size: opts.min_size,
            captured_buttons: Cell::new(0),
            is_tracking_hover: Cell::new(false),
            delegate: RefCell::new(None),
            ime_delegate: RefCell::new(None),
            a11y_delegate: RefCell::new(None),
            composition_active: Cell::new(false),
            in_size_move: Cell::new(false),
            pending_display_change: Cell::new(false),
            logged_no_delegate: Cell::new(false),
            logged_weak_upgrade_none: Cell::new(false),
            dxgi_factory,
            last_monitor: Cell::new(HMONITOR(std::ptr::null_mut())),
            pending_monitor_change: Cell::new(false),
            pending_high_surrogate: Cell::new(None),
            redraw_timer_armed: Cell::new(false),
            releasing_capture: Cell::new(false),
            monitor_luid_override: Cell::new(None),
        });

        let inner_ptr = Arc::as_ptr(&inner);

        let (ex_style, style) = if opts.visible {
            (WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW | WS_VISIBLE)
        } else {
            // Off-screen test harness: borderless, disabled, no taskbar entry.
            // WS_VISIBLE is required for the window to receive WM_PAINT — the
            // window is kept off the user's desktop via `opts.position` (e.g.
            // (-32000, -32000)) so it owns a real DXGI surface without ever
            // being seen.
            (
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW,
                WS_POPUP | WS_DISABLED | WS_VISIBLE,
            )
        };
        let (x, y) = match opts.position {
            Some((x, y)) => (x, y),
            None => (CW_USEDEFAULT, CW_USEDEFAULT),
        };

        // SAFETY: All arguments are valid Win32 values; CLASS_NAME was registered
        // in WinPlatform::new. The lpCreateParams raw pointer is valid for the
        // duration of CreateWindowExW (the user's Arc lives on the stack here).
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                CLASS_NAME,
                PCWSTR(title_w.as_ptr()),
                style,
                x,
                y,
                opts.size.0 as i32,
                opts.size.1 as i32,
                None,
                None,
                Some(hinstance),
                Some(inner_ptr as *const _),
            )
        }
        .expect("CreateWindowExW failed");

        // Patch the HWND into the inner.
        let inner_mut = inner_ptr as *mut WinWindowInner;
        // SAFETY: inner_ptr is our Arc's pointer; no other thread has access yet.
        unsafe { (*inner_mut).hwnd = hwnd };

        // Seed last_monitor with the real HMONITOR so the first
        // WM_WINDOWPOSCHANGED after creation does not fire a spurious
        // change against a null sentinel.
        // SAFETY: hwnd is a valid window handle just returned by CreateWindowExW.
        let initial_hmon = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        inner.last_monitor.set(initial_hmon);

        // Register this HWND for wake events from background threads.
        // First window wins; subsequent calls are no-ops (CAS).
        register_wake_hwnd(hwnd);

        // Register id → HWND so screen-reader actions posted from the UIA
        // adapter (possibly off-thread) resolve to this window's queue.
        register_window_hwnd(id, hwnd);

        // Restore a maximized window (geometry persistence). The visible window
        // was created normal above; maximize it now. CreateWindowExW already
        // positioned the restored normal bounds, which Windows tracks as the
        // un-maximize target.
        if opts.maximized {
            // SAFETY: hwnd is a valid window handle just returned by CreateWindowExW.
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWMAXIMIZED) };
        }

        #[allow(clippy::arc_with_non_send_sync)]
        Arc::new(WinWindow { inner })
    }
}

impl Window for WinWindow {
    fn logical_size(&self) -> (u32, u32) {
        // GetClientRect returns physical pixels on Per-Monitor-Aware-V2; divide by scale.
        let mut rect = RECT::default();
        // SAFETY: hwnd is valid; rect is a valid output pointer.
        let _ = unsafe { GetClientRect(self.inner.hwnd, &mut rect) };
        let scale = self.scale_factor();
        (
            ((rect.right - rect.left) as f64 / scale).round() as u32,
            ((rect.bottom - rect.top) as f64 / scale).round() as u32,
        )
    }

    fn physical_size(&self) -> (u32, u32) {
        let mut rect = RECT::default();
        // SAFETY: hwnd is valid; rect is a valid output pointer.
        let _ = unsafe { GetClientRect(self.inner.hwnd, &mut rect) };
        (
            (rect.right - rect.left) as u32,
            (rect.bottom - rect.top) as u32,
        )
    }

    fn scale_factor(&self) -> f64 {
        // SAFETY: hwnd is valid.
        let dpi = unsafe { GetDpiForWindow(self.inner.hwnd) };
        dpi as f64 / 96.0
    }

    fn request_redraw(&self) {
        // SAFETY: Some(hwnd) is valid; None invalidates the entire client area.
        let _ = unsafe { InvalidateRect(Some(self.inner.hwnd), None, false) };
    }

    fn set_title(&self, title: &str) {
        let title_w = to_wide(title);
        // SAFETY: hwnd is valid; title_w is null-terminated UTF-16.
        let _ = unsafe { SetWindowTextW(self.inner.hwnd, PCWSTR(title_w.as_ptr())) };
    }

    fn close(&self) {
        // SAFETY: hwnd is valid.
        let _ = unsafe { DestroyWindow(self.inner.hwnd) };
    }

    fn id(&self) -> WindowId {
        self.inner.id
    }

    fn set_render_delegate(&self, delegate: Weak<dyn WindowRenderDelegate>) {
        *self.inner.delegate.borrow_mut() = Some(delegate);
    }

    fn set_ime_delegate(&self, delegate: Weak<dyn WindowImeDelegate>) {
        *self.inner.ime_delegate.borrow_mut() = Some(delegate);
    }

    fn set_a11y_delegate(&self, delegate: Weak<dyn WindowA11yDelegate>) {
        *self.inner.a11y_delegate.borrow_mut() = Some(delegate);
    }

    fn current_monitor_luid(&self) -> Option<u64> {
        // Test override (always None in production) lets a test force a
        // deterministic adapter-LUID mismatch without a second physical GPU.
        if let Some(luid) = self.inner.monitor_luid_override.get() {
            return Some(luid);
        }
        let factory = self.inner.dxgi_factory.as_ref()?;
        // SAFETY: hwnd is valid for the lifetime of this WinWindow.
        let hmon = unsafe { MonitorFromWindow(self.inner.hwnd, MONITOR_DEFAULTTONEAREST) };
        if hmon.is_invalid() {
            return None;
        }
        // SAFETY: factory is a live IDXGIFactory2; EnumAdapters/EnumOutputs/GetDesc
        // return COM error on out-of-range, breaking the loops normally. HMONITOR
        // equality compares raw pointer values.
        unsafe {
            for adapter_idx in 0u32.. {
                let Ok(adapter) = factory.EnumAdapters(adapter_idx) else {
                    break;
                };
                for output_idx in 0u32.. {
                    let Ok(output) = adapter.EnumOutputs(output_idx) else {
                        break;
                    };
                    let Ok(odesc) = output.GetDesc() else {
                        continue;
                    };
                    if odesc.Monitor == hmon {
                        let adesc = adapter.GetDesc().ok()?;
                        // HighPart is i32 — zero-extend via u32 to avoid
                        // sign-extension corrupting the upper bits.
                        return Some(
                            ((adesc.AdapterLuid.HighPart as u32 as u64) << 32)
                                | (adesc.AdapterLuid.LowPart as u64),
                        );
                    }
                }
            }
        }
        None
    }

    fn in_size_move(&self) -> bool {
        self.inner.in_size_move.get()
    }

    fn set_menu(&self, menu: &PlatformMenu) {
        super::menu::set_menu_bar(self.inner.hwnd, menu);
    }

    fn show_context_menu(&self, menu: &PlatformMenu, at: (f32, f32)) {
        // `at` is logical view points (top-left origin); TrackPopupMenu works in
        // physical pixels. Scale to client pixels here; the menu backend handles
        // the client→screen conversion.
        let scale = self.scale_factor() as f32;
        let client = ((at.0 * scale).round() as i32, (at.1 * scale).round() as i32);
        super::menu::pop_up_context_menu(self.inner.hwnd, menu, client);
    }

    fn position(&self) -> Option<(i32, i32)> {
        // `rcNormalPosition` is the restored (un-maximized) frame in workspace
        // coordinates — the bounds Windows returns the window to when
        // un-maximized. Persisting this means a window saved while maximized
        // still has sane normal bounds on restore. Round-trips with the screen
        // coordinates `CreateWindowExW` consumes via `WindowOptions::position`.
        let mut placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: hwnd is valid; `placement` is a correctly-sized output struct.
        unsafe { GetWindowPlacement(self.inner.hwnd, &mut placement).ok()? };
        Some((
            placement.rcNormalPosition.left,
            placement.rcNormalPosition.top,
        ))
    }

    fn placement(&self) -> WindowPlacement {
        let mut placement = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        // SAFETY: hwnd is valid; `placement` is a correctly-sized output struct.
        if unsafe { GetWindowPlacement(self.inner.hwnd, &mut placement) }.is_err() {
            return WindowPlacement::Normal;
        }
        // `showCmd` is a `u32`; the `SW_*` constants are `SHOW_WINDOW_CMD(i32)`.
        if placement.showCmd == SW_SHOWMAXIMIZED.0 as u32 {
            WindowPlacement::Maximized
        } else if placement.showCmd == SW_SHOWMINIMIZED.0 as u32 {
            WindowPlacement::Minimized
        } else {
            WindowPlacement::Normal
        }
    }

    fn is_live_resizing(&self) -> bool {
        self.inner.in_size_move.get()
    }

    fn schedule_redraw_at(&self, deadline: std::time::Instant) {
        let now = std::time::Instant::now();
        // Win32 `SetTimer` takes a `u32` millisecond interval and treats 0 as
        // "fire ASAP." Clamp to `USER_TIMER_MINIMUM` (10ms) so the timer is
        // not coalesced with WM_PAINT into the same iteration when callers
        // pass a deadline that's already past, and saturate to `u32::MAX` to
        // avoid silent truncation on huge deadlines.
        let ms: u32 = if deadline > now {
            let micros = deadline.duration_since(now).as_micros();
            ((micros / 1000) as u64).clamp(USER_TIMER_MINIMUM as u64, u32::MAX as u64) as u32
        } else {
            USER_TIMER_MINIMUM
        };
        // Replace an in-flight timer so a second call before fire does not
        // double-schedule. Per-window single timer id (`REDRAW_TIMER_ID`).
        if self.inner.redraw_timer_armed.get() {
            // SAFETY: hwnd is valid for the lifetime of this WinWindow.
            // KillTimer is idempotent on already-fired ids.
            let _ = unsafe { KillTimer(Some(self.inner.hwnd), REDRAW_TIMER_ID) };
        }
        // SAFETY: hwnd is valid; lpTimerFunc=None routes via WM_TIMER.
        let assigned = unsafe { SetTimer(Some(self.inner.hwnd), REDRAW_TIMER_ID, ms, None) };
        self.inner.redraw_timer_armed.set(assigned != 0);
        if assigned == 0 {
            log::warn!(
                target: "slate::win",
                "SetTimer failed in schedule_redraw_at; redraw deferred to next natural event"
            );
        }
    }

    fn focus(&self) {
        // Raise to the foreground. Each Windows window owns its own `HMENU`
        // (per-`HWND` `SetMenu`), so no menu swap is needed on focus.
        // SAFETY: hwnd is valid for the lifetime of this WinWindow.
        let _ = unsafe { SetForegroundWindow(self.inner.hwnd) };
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl WinWindow {
    #[doc(hidden)]
    pub fn set_in_size_move_for_test(&self, v: bool) {
        self.inner.in_size_move.set(v);
    }

    /// Override what `current_monitor_luid` reports. Lets a test drive a
    /// deterministic adapter-LUID mismatch (window vs renderer) without a
    /// second physical GPU. `None` restores the real DXGI probe.
    #[doc(hidden)]
    pub fn set_monitor_luid_for_test(&self, luid: Option<u64>) {
        self.inner.monitor_luid_override.set(luid);
    }

    /// True iff a redraw timer scheduled via `schedule_redraw_at` is currently
    /// armed (between `SetTimer` and either `WM_TIMER` fire or replacement).
    /// Test-only — operator stress tests assert this returns to `false` after
    /// the message pump drains.
    #[doc(hidden)]
    pub fn redraw_timer_armed_for_test(&self) -> bool {
        self.inner.redraw_timer_armed.get()
    }
}

impl HasWindowHandle for WinWindow {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd_nzi =
            NonZeroIsize::new(self.inner.hwnd.0 as isize).expect("HWND must be non-null");
        let mut handle = Win32WindowHandle::new(hwnd_nzi);
        handle.hinstance = NonZeroIsize::new(self.inner.hinstance.0 as isize);
        // SAFETY: the handle is valid for the duration of the `&self` borrow.
        unsafe { Ok(WindowHandle::borrow_raw(RawWindowHandle::Win32(handle))) }
    }
}

impl HasDisplayHandle for WinWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: Windows display handle is always valid on this OS.
        unsafe {
            Ok(DisplayHandle::borrow_raw(RawDisplayHandle::Windows(
                WindowsDisplayHandle::new(),
            )))
        }
    }
}
