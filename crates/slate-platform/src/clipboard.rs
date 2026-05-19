//! Cross-platform clipboard text access (Phase 10b.2).
//!
//! Plain-text round-trip only. RTF / image / file-list types are ignored per
//! the v2 scope decision (OQ4).
//!
//! # Threading
//!
//! Both functions MUST be called from the main / event-loop thread:
//! - Win32 clipboard ops require a thread that owns a top-level window.
//! - `NSPasteboard` is bound to the AppKit main thread.
//!
//! # Error reporting
//!
//! The signatures return `Option<String>` / nothing. Transient failures
//! (Win32 contention, NSPasteboard return nil) are logged at `warn` and
//! surface as `None` / no-op. The TextField key bindings already tolerate
//! both outcomes.

/// Return the text contents of the system clipboard, if any.
///
/// Returns `None` when the clipboard is empty, holds non-text content, or the
/// OS denied access.
pub fn get_text() -> Option<String> {
    imp::get_text()
}

/// Write `s` to the system clipboard as plain text. Silently no-ops on
/// transient OS errors (clipboard busy, etc).
pub fn set_text(s: &str) {
    imp::set_text(s);
}

// ---------------------------------------------------------------------------
// macOS impl — NSPasteboard
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod imp {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;

    pub(super) fn get_text() -> Option<String> {
        unsafe {
            let pb = NSPasteboard::generalPasteboard();
            let ns_str = pb.stringForType(NSPasteboardTypeString)?;
            Some(ns_str.to_string())
        }
    }

    pub(super) fn set_text(s: &str) {
        unsafe {
            let pb = NSPasteboard::generalPasteboard();
            let _ = pb.clearContents();
            let ns_str = NSString::from_str(s);
            let ok = pb.setString_forType(&ns_str, NSPasteboardTypeString);
            if !ok {
                log::warn!("NSPasteboard setString_forType returned NO");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Windows impl — OpenClipboard / CF_UNICODETEXT
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod imp {
    use std::thread::sleep;
    use std::time::Duration;

    use windows::Win32::Foundation::{HANDLE, HWND};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GHND, GlobalAlloc, GlobalFree, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_UNICODETEXT;

    const RETRY_ATTEMPTS: usize = 5;
    const RETRY_BACKOFF: Duration = Duration::from_millis(10);

    /// Open the clipboard with retry. Returns `true` on success.
    fn open_with_retry() -> bool {
        for _ in 0..RETRY_ATTEMPTS {
            // HWND(0) — null owner; valid for read access on a window-owning thread.
            if unsafe { OpenClipboard(Some(HWND(std::ptr::null_mut()))) }.is_ok() {
                return true;
            }
            sleep(RETRY_BACKOFF);
        }
        false
    }

    pub(super) fn get_text() -> Option<String> {
        if !open_with_retry() {
            log::warn!("OpenClipboard failed after {} retries", RETRY_ATTEMPTS);
            return None;
        }
        let handle = unsafe { GetClipboardData(CF_UNICODETEXT.0 as u32) }.ok();
        let result = handle.and_then(|h| unsafe {
            let ptr = GlobalLock(std::mem::transmute::<HANDLE, _>(h)) as *const u16;
            if ptr.is_null() {
                return None;
            }
            // Walk to NUL terminator.
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
                // Safety belt: cap at 16 MiB UTF-16 (≈32 MiB) — well past any
                // sane clipboard payload, prevents an unbounded scan on a
                // malformed handle.
                if len > 16 * 1024 * 1024 {
                    log::warn!("CF_UNICODETEXT exceeded 16Mi UTF-16 code units");
                    break;
                }
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let s = String::from_utf16_lossy(slice);
            let _ = GlobalUnlock(std::mem::transmute::<HANDLE, _>(h));
            Some(s)
        });
        let _ = unsafe { CloseClipboard() };
        result
    }

    pub(super) fn set_text(s: &str) {
        if !open_with_retry() {
            log::warn!("OpenClipboard failed after {} retries", RETRY_ATTEMPTS);
            return;
        }
        if let Err(e) = unsafe { EmptyClipboard() } {
            log::warn!("EmptyClipboard failed: {e:?}");
            let _ = unsafe { CloseClipboard() };
            return;
        }
        // UTF-16 + NUL terminator.
        let mut utf16: Vec<u16> = s.encode_utf16().collect();
        utf16.push(0);
        let bytes = utf16.len() * 2;
        let hmem = unsafe { GlobalAlloc(GHND, bytes) };
        let hmem = match hmem {
            Ok(h) if !h.0.is_null() => h,
            _ => {
                log::warn!("GlobalAlloc failed for clipboard write");
                let _ = unsafe { CloseClipboard() };
                return;
            }
        };
        unsafe {
            let dst = GlobalLock(hmem) as *mut u16;
            if dst.is_null() {
                let _ = GlobalFree(Some(hmem));
                log::warn!("GlobalLock returned NULL for clipboard write");
                let _ = CloseClipboard();
                return;
            }
            std::ptr::copy_nonoverlapping(utf16.as_ptr(), dst, utf16.len());
            let _ = GlobalUnlock(hmem);
            // On success ownership of hmem transfers to the clipboard.
            let handle = HANDLE(hmem.0);
            if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(handle)).is_err() {
                log::warn!("SetClipboardData failed");
                let _ = GlobalFree(Some(hmem));
            }
        }
        let _ = unsafe { CloseClipboard() };
    }
}

// ---------------------------------------------------------------------------
// Fallback (Linux / headless / other) — stub
// ---------------------------------------------------------------------------

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    pub(super) fn get_text() -> Option<String> {
        None
    }
    pub(super) fn set_text(_s: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On macOS + Windows this exercises the real OS clipboard. The test only
    /// asserts the round-trip when a write succeeds — sandboxed CI without a
    /// pasteboard server lands on the fallback `None` branch and the assertion
    /// is skipped.
    #[test]
    fn set_then_get_round_trips_when_clipboard_available() {
        let probe = "slate-clipboard-test::round-trip";
        set_text(probe);
        match get_text() {
            Some(got) if got == probe => {} // happy path
            Some(other) => panic!("clipboard payload mismatch: got {other:?}"),
            None => {
                // Headless / fallback path. Acceptable.
            }
        }
    }
}
