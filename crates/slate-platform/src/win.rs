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
//! # Arc ownership protocol (C1 / C2 fix)
//! The user receives `Arc<WinWindow>` (which wraps `Arc<WinWindowInner>`). The OS
//! holds a `*const WinWindowInner` in `GWLP_USERDATA`. At `WM_NCCREATE` we call
//! `Arc::increment_strong_count` to give the OS a logical Arc reference. At
//! `WM_NCDESTROY` (the documented last-ever message for an HWND) we call
//! `Arc::decrement_strong_count` and clear `GWLP_USERDATA`. No `Box::into_raw`
//! of a duplicate state; single source of truth.

use std::cell::Cell;
use std::num::NonZeroIsize;
use std::sync::Arc;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
};
use windows::Win32::Foundation::{
    ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM,
};
use windows::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForWindow, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_OWNDC, CS_VREDRAW, CW_USEDEFAULT, CREATESTRUCTW, CreateWindowExW,
    DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, GetMessageW,
    GetWindowLongPtrW, GWLP_USERDATA, IDC_ARROW, LoadCursorW, MSG, PostQuitMessage,
    RegisterClassExW, SetWindowLongPtrW, SetWindowPos, SetWindowTextW,
    SWP_NOACTIVATE, SWP_NOZORDER, TranslateMessage, WINDOW_EX_STYLE, WNDCLASSEXW, WM_CLOSE,
    WM_DESTROY, WM_DPICHANGED, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WM_SIZE,
    WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};
use windows::core::{w, PCWSTR};

use crate::{Event, Platform, Window, WindowId, WindowOptions};

// ---------------------------------------------------------------------------
// Thread-local event handler storage (lifetime-erasure pattern — mirrors mac.rs)
// ---------------------------------------------------------------------------

type EventHandler = std::cell::RefCell<Option<Box<dyn FnMut(Event) + 'static>>>;

thread_local! {
    static HANDLER: EventHandler = const { std::cell::RefCell::new(None) };
    static NEXT_WINDOW_ID: Cell<u64> = const { Cell::new(1) };
}

fn dispatch_event(event: Event) {
    HANDLER.with(|h| {
        if let Some(handler) = h.borrow_mut().as_mut() {
            handler(event);
        }
    });
}

fn next_window_id() -> WindowId {
    NEXT_WINDOW_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        WindowId(id)
    })
}

// ---------------------------------------------------------------------------
// Wide-string helper
// ---------------------------------------------------------------------------

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------
// Window class name (compile-time wide literal)
// ---------------------------------------------------------------------------

const CLASS_NAME: PCWSTR = w!("SlateWindowClass");

// ---------------------------------------------------------------------------
// WndProc trampoline — extern "system" callback with catch_unwind + abort (C4)
// ---------------------------------------------------------------------------

/// # SAFETY
/// This is the Win32 window procedure. Called by the OS on the main thread.
/// All Rust dispatch is wrapped in `catch_unwind`; any panic aborts the process
/// rather than unwinding into Win32 (which is UB on stable Rust).
unsafe extern "system" fn wnd_proc_trampoline(
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
            // SAFETY: inner_ptr was created by Arc::as_ptr and is still valid
            // (the user's Arc is alive for the entire CreateWindowExW call).
            unsafe { Arc::increment_strong_count(inner_ptr) };
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, inner_ptr as isize) };
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }

        let inner_ptr =
            unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const WinWindowInner;
        if inner_ptr.is_null() {
            return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
        }

        // SAFETY: GWLP_USERDATA is non-null only between WM_NCCREATE and
        // WM_NCDESTROY (where we clear it). Strong count >= 1 throughout.
        let inner = unsafe { &*inner_ptr };
        let res = inner.handle_message(hwnd, msg, wparam, lparam);

        if msg == WM_NCDESTROY {
            // C1 fix: last message this HWND will ever receive.
            // Clear GWLP_USERDATA first, then drop the OS's Arc reference.
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
            // SAFETY: mirrors the increment_strong_count in WM_NCCREATE.
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

// ---------------------------------------------------------------------------
// WinWindowInner — actual HWND state (Arc'd, OS holds raw ptr via GWLP_USERDATA)
// ---------------------------------------------------------------------------

pub struct WinWindowInner {
    hwnd: HWND,
    hinstance: HINSTANCE,
    id: WindowId,
}

impl WinWindowInner {
    /// Translate a Win32 message into a Slate [`Event`] and dispatch it.
    fn handle_message(&self, hwnd: HWND, msg: u32, _wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_CLOSE => {
                dispatch_event(Event::WindowCloseRequested { window: self.id });
                LRESULT(0)
            }
            WM_DESTROY => {
                // Children may still be tearing down; defer Arc cleanup to WM_NCDESTROY.
                dispatch_event(Event::WindowDestroyed { window: self.id });
                // SAFETY: PostQuitMessage is always safe to call from a WM_DESTROY handler.
                unsafe { PostQuitMessage(0) };
                LRESULT(0)
            }
            WM_SIZE => {
                // H7 fix: narrow LPARAM (i64 on x64) to u32 BEFORE masking.
                // Direct mask on i64 leaks high 32 bits on x64 Windows.
                let lp = lparam.0 as u32;
                let w = lp & 0xFFFF;
                let h = (lp >> 16) & 0xFFFF;
                dispatch_event(Event::WindowResized {
                    window: self.id,
                    size: (w, h),
                });
                LRESULT(0)
            }
            WM_PAINT => {
                dispatch_event(Event::WindowRedrawRequested { window: self.id });
                // SAFETY: Some(hwnd) is valid; None means entire client rect.
                let _ = unsafe { ValidateRect(Some(hwnd), None) };
                LRESULT(0)
            }
            WM_DPICHANGED => {
                // M1 fix: lParam holds a RECT* with the suggested window position/size.
                // Resize to it, then dispatch WindowResized so the renderer reconfigures.
                //
                // SAFETY: Win32 guarantees lParam is a valid *const RECT for WM_DPICHANGED.
                let suggested = unsafe { &*(lparam.0 as *const RECT) };
                let w = (suggested.right - suggested.left) as u32;
                let h = (suggested.bottom - suggested.top) as u32;
                // SAFETY: hwnd is valid; None = no insert-after; flags suppress reorder/activate.
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
                dispatch_event(Event::WindowResized {
                    window: self.id,
                    size: (w, h),
                });
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
// WinWindow — public window handle (thin Arc wrapper)
// ---------------------------------------------------------------------------

/// A native Win32 window.
///
/// Not `Send` or `Sync`; must be used exclusively on the main OS thread.
pub struct WinWindow {
    inner: Arc<WinWindowInner>,
}

impl WinWindow {
    fn new(opts: &WindowOptions, hinstance: HINSTANCE) -> Arc<Self> {
        let id = next_window_id();
        let title_w = to_wide(&opts.title);

        // Build the inner state first so we can pass a stable raw pointer to
        // CreateWindowExW as lpCreateParams. WinWindowInner is intentionally
        // main-thread-only (not Send+Sync); suppress the arc_with_non_send_sync lint.
        #[allow(clippy::arc_with_non_send_sync)]
        let inner = Arc::new(WinWindowInner {
            // hwnd is filled in after CreateWindowExW returns (see below).
            // Use a placeholder; the field is set immediately after.
            hwnd: HWND(std::ptr::null_mut()),
            hinstance,
            id,
        });

        // Pass the raw pointer as lpCreateParams. WM_NCCREATE fires synchronously
        // during CreateWindowExW and calls Arc::increment_strong_count on this ptr.
        let inner_ptr = Arc::as_ptr(&inner);

        // SAFETY: All arguments are valid Win32 values; CLASS_NAME was registered
        // in WinPlatform::new. The lpCreateParams raw pointer is valid for the
        // duration of CreateWindowExW (the user's Arc lives on the stack here).
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
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

        // Patch the HWND into the inner. SAFETY: we have exclusive ownership of
        // `inner` here (no other Arc clone exists yet; the OS-side reference was
        // incremented inside WM_NCCREATE which ran on this thread synchronously).
        // We cast away the const to write the hwnd; this is safe because we have
        // the only Rust-side Arc reference at this point.
        let inner_mut = inner_ptr as *mut WinWindowInner;
        // SAFETY: inner_ptr is our Arc's pointer; no other thread has access yet.
        unsafe { (*inner_mut).hwnd = hwnd };

        // The Platform trait mandates Arc<Self::Window>; suppress arc_with_non_send_sync
        // because WinWindow is intentionally main-thread-only.
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
        // SAFETY: hwnd.0 is the Win32 HWND value; NonZeroIsize is valid because
        // CreateWindowExW returns a non-null handle (or we would have panicked).
        let hwnd_nzi = NonZeroIsize::new(self.inner.hwnd.0 as isize)
            .expect("HWND must be non-null");
        let mut handle = Win32WindowHandle::new(hwnd_nzi);
        handle.hinstance = NonZeroIsize::new(self.inner.hinstance.0 as isize);
        // SAFETY: the handle is valid for the duration of the `&self` borrow.
        unsafe { Ok(WindowHandle::borrow_raw(RawWindowHandle::Win32(handle))) }
    }
}

impl HasDisplayHandle for WinWindow {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        // WindowsDisplayHandle carries no pointer; always valid on Windows.
        // SAFETY: Windows display handle is always valid on this OS.
        unsafe {
            Ok(DisplayHandle::borrow_raw(RawDisplayHandle::Windows(
                WindowsDisplayHandle::new(),
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// WinPlatform — public platform handle
// ---------------------------------------------------------------------------

/// The Win32 platform driver.
///
/// Must be created and used exclusively on the main OS thread.
pub struct WinPlatform {
    hinstance: HINSTANCE,
    /// Guards against re-entrant calls to `run`.
    running: Cell<bool>,
}

impl Platform for WinPlatform {
    type Window = WinWindow;

    fn new() -> Self {
        // H6 fix: enable per-monitor v2 DPI awareness once per process.
        // SAFETY: Always safe to call; failure (already set) is non-fatal.
        let _ = unsafe {
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2)
        };

        // SAFETY: None means "current module"; always succeeds for a running process.
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None) }
            .expect("GetModuleHandleW failed")
            .into();

        // H3 fix: register window class; idempotent under ERROR_CLASS_ALREADY_EXISTS.
        // SAFETY: All WNDCLASSEXW fields are valid Win32 values.
        let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }
            .expect("LoadCursorW failed");

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_OWNDC,
            lpfnWndProc: Some(wnd_proc_trampoline),
            hInstance: hinstance,
            lpszClassName: CLASS_NAME,
            hCursor: cursor,
            ..Default::default()
        };

        // SAFETY: &wc is a valid WNDCLASSEXW pointer.
        let atom = unsafe { RegisterClassExW(&wc) };
        if atom == 0 {
            // SAFETY: GetLastError is always safe to call after a Win32 failure.
            let err = unsafe { GetLastError() };
            if err != ERROR_CLASS_ALREADY_EXISTS {
                panic!("RegisterClassExW failed: {err:?}");
            }
            // Class registered by a previous WinPlatform instance — idempotent.
        }

        WinPlatform {
            hinstance,
            running: Cell::new(false),
        }
    }

    fn create_window(&self, opts: WindowOptions) -> Arc<WinWindow> {
        WinWindow::new(&opts, self.hinstance)
    }

    fn run<F>(&self, mut handler: F)
    where
        F: FnMut(Event),
    {
        assert!(!self.running.get(), "WinPlatform::run called re-entrantly");
        self.running.set(true);

        // Erase the handler's lifetime to 'static for HANDLER thread-local storage.
        //
        // SAFETY:
        //   (1) The GetMessageW pump below blocks this call stack until WM_QUIT.
        //   (2) All Win32 callbacks (wnd_proc_trampoline) that access HANDLER
        //       execute on this same main thread; no concurrent access.
        //   (3) We unconditionally clear HANDLER before returning, so the raw
        //       pointer embedded in the Box is never dereferenced after the
        //       closure's borrow ends.
        //   (4) Therefore the transmute is equivalent to lending a &'run_scope
        //       reference to a scope strictly nested within run_scope.
        let handler_static: Box<dyn FnMut(Event) + 'static> = {
            let erased: Box<dyn FnMut(Event) + '_> = Box::new(&mut handler);
            // SAFETY: see argument above.
            unsafe { std::mem::transmute(erased) }
        };

        HANDLER.with(|h| {
            *h.borrow_mut() = Some(handler_static);
        });

        // Emit Resumed before entering the pump (mirrors AppKit's applicationDidFinishLaunching).
        dispatch_event(Event::Resumed);

        // Message pump — blocks until WM_QUIT.
        let mut msg = MSG::default();
        // SAFETY: &mut msg is a valid output pointer; None = all windows on this thread.
        while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
            // SAFETY: msg is a valid MSG from GetMessageW.
            let _ = unsafe { TranslateMessage(&msg) };
            // SAFETY: msg is a valid MSG from GetMessageW.
            unsafe { DispatchMessageW(&msg) };
        }

        // Run loop has exited; emit Exiting while handler is still live.
        dispatch_event(Event::Exiting);

        // CRITICAL: clear the handler to invalidate the 'static-erased pointer
        // BEFORE this function returns and the borrow of `handler` ends.
        HANDLER.with(|h| {
            *h.borrow_mut() = None;
        });

        self.running.set(false);
    }

    fn quit(&self) {
        // Idempotent: PostQuitMessage is safe to call even outside a run loop.
        // SAFETY: Always safe; 0 is the conventional exit code.
        unsafe { PostQuitMessage(0) };
    }
}
