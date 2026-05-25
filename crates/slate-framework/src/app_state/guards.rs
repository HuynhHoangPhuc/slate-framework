//! RAII guards for transient per-call flags on `AppState`.
//!
//! `RenderingGuard` covers the re-entrancy lock around `run_redraw`.
//! `SyncResizeGuard` keeps the sync-resize flag true for the duration of the
//! AppKit selector that opened the resize CATransaction.
//! `reset_borrow_order` resets the debug-only borrow-order tracker (ADR-001).

use std::cell::Cell;

// Debug-mode borrow-order discipline (see ADR-001).
// Detects borrow-order violations before they ship.
#[cfg(debug_assertions)]
thread_local! {
    static BORROW_ORDER: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

#[cfg(debug_assertions)]
pub(super) fn reset_borrow_order() {
    BORROW_ORDER.with(|c| c.set(0));
}

#[cfg(not(debug_assertions))]
pub(super) fn reset_borrow_order() {}

/// RAII guard for the rendering flag.
pub(super) struct RenderingGuard<'a>(pub(super) &'a Cell<bool>);

impl Drop for RenderingGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

/// RAII guard that holds the sync-resize flag true for the duration of the
/// AppKit selector that opened the resize CATransaction, then clears it
/// unconditionally on Drop so a panic in the resize/dispatch path cannot
/// leave subsequent normal redraws stuck on the sync-present codepath.
pub(super) struct SyncResizeGuard<'a>(&'a Cell<bool>);

impl<'a> SyncResizeGuard<'a> {
    pub(super) fn new(flag: &'a Cell<bool>) -> Self {
        flag.set(true);
        Self(flag)
    }
}

impl Drop for SyncResizeGuard<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}
