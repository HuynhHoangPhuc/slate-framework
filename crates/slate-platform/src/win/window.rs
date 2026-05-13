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
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DestroyWindow, GetClientRect, SetWindowTextW,
    WS_DISABLED, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP,
    WS_VISIBLE,
};
use windows::core::PCWSTR;

use super::message_loop::WinWindowInner;
use super::platform::CLASS_NAME;
use super::{next_window_id, register_wake_hwnd, to_wide};
use crate::{Window, WindowId, WindowOptions, WindowRenderDelegate};

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
        let title_w = to_wide(&opts.title);

        #[allow(clippy::arc_with_non_send_sync)]
        let inner = Arc::new(WinWindowInner {
            hwnd: HWND(std::ptr::null_mut()),
            hinstance,
            id,
            min_size: opts.min_size,
            captured_buttons: Cell::new(0),
            is_tracking_hover: Cell::new(false),
            delegate: RefCell::new(None),
            in_size_move: Cell::new(false),
            pending_display_change: Cell::new(false),
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

        // Register this HWND for wake events from background threads.
        // First window wins; subsequent calls are no-ops (CAS).
        register_wake_hwnd(hwnd);

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
