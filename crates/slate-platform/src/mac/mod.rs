//! macOS platform backend using `objc2` + AppKit.
//!
//! # Thread safety
//! All types in this module are **main-thread-only**. `MacWindow` is
//! deliberately not `Send` or `Sync`; AppKit `NSWindow`/`NSView` may only be
//! touched on the thread that created them.
//!
//! # Handler lifetime
//! `MacPlatform::run` accepts a `FnMut(Event)` of any lifetime. Internally
//! it erases the lifetime to `'static` for storage in a `thread_local!`
//! `RefCell`. This is sound because `run` **blocks** for the entire duration
//! of the handler's borrow: the `'static`-cast pointer is cleared before
//! `run` returns, so the raw pointer can never outlive the closure.
//! See the SAFETY comment in `run` for the full argument.

mod platform;
mod view;
mod window;

pub use platform::MacPlatform;
pub use window::MacWindow;

use std::cell::Cell;
use std::panic::AssertUnwindSafe;
use std::sync::OnceLock;
use std::time::Instant;

use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::{MainThreadMarker, NSPoint};

use crate::{Event, WindowId};

// ---------------------------------------------------------------------------
// Thread-local event handler storage
// ---------------------------------------------------------------------------

type EventHandler = std::cell::RefCell<Option<Box<dyn FnMut(Event) + 'static>>>;

/// Subtype marker for synthetic ApplicationDefined events that carry a redraw
/// request. `data1` encodes the target `WindowId`.
pub(crate) const REDRAW_EVENT_SUBTYPE: i16 = 42;

/// Subtype marker for wake events from background threads.
pub(crate) const WAKE_EVENT_SUBTYPE: i16 = 43;

thread_local! {
    pub(crate) static HANDLER: EventHandler = const { std::cell::RefCell::new(None) };
}

/// Dispatch an `Event` through the thread-local handler. No-op if none installed.
pub(crate) fn dispatch_event(event: Event) {
    HANDLER.with(|h| {
        if let Some(handler) = h.borrow_mut().as_mut() {
            handler(event);
        }
    });
}

/// Post a synthetic event to the run loop that will be processed on the next
/// iteration. Used by `request_redraw` to defer redraw delivery and avoid
/// RefCell re-entrancy (safe to call from within a handler).
pub(crate) fn post_redraw_event(window_id: WindowId) {
    let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        REDRAW_EVENT_SUBTYPE,
        window_id.0 as isize,
        0,
    );
    if let Some(event) = event {
        let mtm = MainThreadMarker::new().unwrap();
        NSApplication::sharedApplication(mtm).postEvent_atStart(&event, false);
    }
}

/// Wake the main run loop from a background thread.
///
/// Thread-safe. Posts a synthetic event that triggers `Event::Wake`.
/// Used by background executors to signal task completion.
pub fn wake_run_loop() {
    let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
        NSEventType::ApplicationDefined,
        NSPoint::new(0.0, 0.0),
        NSEventModifierFlags::empty(),
        0.0,
        0,
        None,
        WAKE_EVENT_SUBTYPE,
        0,
        0,
    );
    if let Some(event) = event {
        // SAFETY: `postEvent:atStart:` is documented as thread-safe by Apple:
        // "You can call this method from any thread of your app."
        // See: https://developer.apple.com/documentation/appkit/nsapplication/postevent(_:atstart:)
        //
        // We use `MainThreadMarker::new_unchecked()` because objc2 requires it
        // for `sharedApplication`, but we only call the thread-safe `postEvent`
        // method — we don't access any main-thread-only state.
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        NSApplication::sharedApplication(mtm).postEvent_atStart(&event, false);
    }
}

/// Abort-safe wrapper for every Rust body reachable from AppKit dispatch.
///
/// Unwinding across an `extern "C"` boundary is undefined behavior on stable
/// Rust. Every Obj-C delegate method / view callback must call this wrapper
/// so that a Rust panic aborts the process loudly instead of silently
/// corrupting AppKit state.
///
/// We use `AssertUnwindSafe` because the closures capture `&self` whose ivars
/// contain `Cell<T>` (not `UnwindSafe`). This is acceptable: we abort on any
/// panic, so we never observe the inconsistent state that would follow from a
/// non-unwind-safe unwinding path.
pub(crate) fn ffi_boundary(f: impl FnOnce()) {
    if std::panic::catch_unwind(AssertUnwindSafe(f)).is_err() {
        log::error!("slate: Rust panic crossed AppKit FFI boundary — aborting");
        std::process::abort();
    }
}

/// Nanoseconds elapsed since process start (for resize trace logging).
pub(crate) fn elapsed_ns() -> u128 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos()
}

// ---------------------------------------------------------------------------
// Monotonic WindowId counter (main-thread-only → Cell is fine)
// ---------------------------------------------------------------------------

thread_local! {
    static NEXT_WINDOW_ID: Cell<u64> = const { Cell::new(1) };
}

pub(crate) fn next_window_id() -> WindowId {
    NEXT_WINDOW_ID.with(|c| {
        let id = c.get();
        c.set(id + 1);
        WindowId(id)
    })
}
