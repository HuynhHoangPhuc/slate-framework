//! WinWindow — native Win32 window handle.

use std::cell::Cell;
use std::num::NonZeroIsize;
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
    WS_EX_NOREDIRECTIONBITMAP, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::PCWSTR;

use super::message_loop::WinWindowInner;
use super::platform::CLASS_NAME;
use super::{next_window_id, register_wake_hwnd, to_wide};
use crate::{Window, WindowId, WindowOptions};

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
        });

        let inner_ptr = Arc::as_ptr(&inner);

        // SAFETY: All arguments are valid Win32 values; CLASS_NAME was registered
        // in WinPlatform::new. The lpCreateParams raw pointer is valid for the
        // duration of CreateWindowExW (the user's Arc lives on the stack here).
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP,
                CLASS_NAME,
                PCWSTR(title_w.as_ptr()),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
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
    fn size(&self) -> (u32, u32) {
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
