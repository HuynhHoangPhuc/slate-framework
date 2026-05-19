//! Cross-platform clipboard access — Phase 10a.8 stubs.
//!
//! Real impls land in Phase 10b.2:
//! - Windows: `OpenClipboard` / `GetClipboardData(CF_UNICODETEXT)` /
//!   `SetClipboardData` round-trip with `EmptyClipboard` + global-alloc
//!   discipline.
//! - macOS: `NSPasteboard::generalPasteboard` `stringForType:NSPasteboardTypeString`
//!   and `setString:forType:`.
//! - Linux / headless: today the stub IS the fallback — `None` / no-op.
//!   X11/Wayland lands post-Phase-0.
//!
//! For 10a the surface exists so the editor crate can call into it without
//! cfg-fences and without taking a hard dependency on the eventual platform
//! types. `get_text` returns `None`; `set_text` is a no-op. These signatures
//! are the contract that 10b.2 implements; callers wired in 10a (Cmd+X /
//! Cmd+C / Cmd+V bindings) will be no-ops on every platform until then.
//!
//! # Threading
//!
//! Both functions MUST be called from the main / event-loop thread. Win32
//! clipboard ops require a window-owning thread; `NSPasteboard` is bound
//! to the AppKit main thread. 10b.2 will `debug_assert!` this — wire
//! callers from the event-loop thread now to avoid a forced refactor.
//!
//! # Error reporting
//!
//! The signature returns `Option<String>` (no `Result`). 10b.2 may need to
//! widen to `Result<Option<String>, ClipboardError>` to surface Win32
//! contention failures (`OpenClipboard` denial under another owner).
//! Decision is deferred until the real-impl review.

/// Return the text contents of the system clipboard, if any.
///
/// **10a stub:** always returns `None`.
pub fn get_text() -> Option<String> {
    None
}

/// Write `s` to the system clipboard as plain text.
///
/// **10a stub:** discards the input.
pub fn set_text(_s: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_get_returns_none() {
        assert!(get_text().is_none());
    }

    /// Forward-spec test: today asserts the stub's "no-op write" contract;
    /// when 10b.2 wires real impls, this test must be flipped to assert the
    /// round-trip (`set_text(s); get_text() == Some(s)`).
    #[test]
    fn set_text_then_get_text_round_trips() {
        set_text("hello");
        assert!(
            get_text().is_none(),
            "stub must not retain text; flip this assertion when 10b.2 lands"
        );
    }
}
