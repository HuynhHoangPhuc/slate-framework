//! WinPlatform — Win32 platform driver.

use std::cell::Cell;
use std::sync::Arc;

use windows::Win32::Foundation::{ERROR_CLASS_ALREADY_EXISTS, GetLastError, HINSTANCE};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_OWNDC, DispatchMessageW, GetMessageW, IDC_ARROW, LoadCursorW, MSG, RegisterClassExW,
    TranslateMessage, WNDCLASSEXW,
};
use windows::core::{PCWSTR, w};

use super::message_loop::wnd_proc_trampoline;
use super::window::WinWindow;
use super::{HANDLER, dispatch_event};
use crate::{Event, Platform, WindowOptions};

// ---------------------------------------------------------------------------
// Window class name (compile-time wide literal)
// ---------------------------------------------------------------------------

pub(crate) const CLASS_NAME: PCWSTR = w!("SlateWindowClass");

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
        let _ =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };

        // SAFETY: None means "current module"; always succeeds for a running process.
        let hinstance: HINSTANCE = unsafe { GetModuleHandleW(None) }
            .expect("GetModuleHandleW failed")
            .into();

        // H3 fix: register window class; idempotent under ERROR_CLASS_ALREADY_EXISTS.
        // SAFETY: All WNDCLASSEXW fields are valid Win32 values.
        let cursor = unsafe { LoadCursorW(None, IDC_ARROW) }.expect("LoadCursorW failed");

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_OWNDC,
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
        loop {
            // SAFETY: &mut msg is a valid output pointer; None = all windows on this thread.
            let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
            if ret.0 == 0 {
                break;
            }
            if ret.0 == -1 {
                log::error!("slate: GetMessageW returned -1; aborting message pump");
                break;
            }
            // SAFETY: msg is a valid MSG from GetMessageW (ret > 0).
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
        // SAFETY: Always safe; 0 is the conventional exit code.
        unsafe { windows::Win32::UI::WindowsAndMessaging::PostQuitMessage(0) };
    }
}
