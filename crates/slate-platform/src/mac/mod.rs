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

mod display_link;
mod keymap;
mod platform;
mod text_offset;
mod view;
mod view_ime;
mod window;

pub use platform::MacPlatform;
pub use window::MacWindow;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
use objc2_foundation::{MainThreadMarker, NSPoint};

use crate::{Event, WindowId, WindowImeDelegate, WindowRenderDelegate};

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
    /// Window registry: lookup MacWindow by WindowId for render delegate dispatch.
    pub(crate) static WINDOWS: RefCell<HashMap<WindowId, std::sync::Weak<MacWindow>>> =
        RefCell::new(HashMap::new());
}

/// Dispatch an `Event` through the thread-local handler. No-op if none installed.
///
/// `try_borrow_mut` tolerates re-entrant dispatch. `AppContext::create_window`
/// can drive synchronous AppKit callbacks (window-shown, frame-change,
/// view-did-move-to-window) that funnel back into `dispatch_event` while the
/// outer handler still holds the borrow. A `borrow_mut` here would panic and
/// abort the process. Re-entrant events are dropped with a trace log; the
/// dynamic-create path republishes the window state post-unwind so no
/// essential signal is lost.
pub(crate) fn dispatch_event(event: Event) {
    HANDLER.with(|h| match h.try_borrow_mut() {
        Ok(mut guard) => {
            if let Some(handler) = guard.as_mut() {
                handler(event);
            }
        }
        Err(_) => {
            log::trace!(
                target: "slate::mac",
                "dispatch_event: re-entrant call, event dropped: {:?}",
                event
            );
        }
    });
}

/// Install an event handler directly into the thread-local slot, bypassing the
/// native run loop. Lets a test exercise the `dispatch_event` funnel — the same
/// path every real AppKit callback uses — without standing up an NSWindow.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn install_event_handler_for_test(handler: Box<dyn FnMut(Event) + 'static>) {
    HANDLER.with(|h| *h.borrow_mut() = Some(handler));
}

/// Push an event through the real dispatch funnel from a test.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn dispatch_event_for_test(event: Event) {
    dispatch_event(event);
}

/// Clear the test-installed handler so a later test starts clean.
#[cfg(feature = "test-hooks")]
#[doc(hidden)]
pub fn clear_event_handler_for_test() {
    HANDLER.with(|h| *h.borrow_mut() = None);
}

/// Register a window in the thread-local registry.
pub(crate) fn register_window(id: WindowId, window: &Arc<MacWindow>) {
    WINDOWS.with(|m| m.borrow_mut().insert(id, Arc::downgrade(window)));
}

/// Unregister a window from the thread-local registry. Idempotent.
pub(crate) fn unregister_window(id: WindowId) {
    WINDOWS.with(|m| {
        m.borrow_mut().remove(&id);
    });
}

/// Lookup MacWindow by id, then upgrade its ime_delegate Weak, then invoke.
/// All steps drop their borrows before invoking — re-entrancy safe.
///
/// Mirrors [`with_window_delegate`]; required by `MetalView`'s
/// `NSTextInputClient` sync queries (`firstRectForCharacterRange:`,
/// `selectedRange`, etc.). Delegate impls (`AppState`) read from the
/// pre-published `cached_ime_query` snapshot (cache-then-query contract).
///
/// Returns the closure's `R` (or `None` if no delegate is wired).
pub(crate) fn with_window_ime_delegate<R>(
    id: WindowId,
    f: impl FnOnce(&dyn WindowImeDelegate) -> R,
) -> Option<R> {
    let window = WINDOWS.with(|m| m.borrow().get(&id).and_then(|w| w.upgrade()))?;
    let weak = window.ime_delegate.borrow().clone()?;
    drop(window);
    let strong = weak.upgrade()?;
    Some(f(&*strong))
}

/// Lookup MacWindow by id, then upgrade its render_delegate Weak, then invoke.
/// All steps drop their borrows before invoking — re-entrancy safe.
pub(crate) fn with_window_delegate(id: WindowId, f: impl FnOnce(&dyn WindowRenderDelegate)) {
    // Step 1: upgrade Weak<MacWindow> from registry; release WINDOWS borrow.
    let window = WINDOWS.with(|m| m.borrow().get(&id).and_then(|w| w.upgrade()));
    let Some(window) = window else {
        return;
    };

    // Step 2: clone Option<Weak<dyn WindowRenderDelegate>> from MacWindow;
    // release render_delegate RefCell borrow IMMEDIATELY.
    let weak = window.render_delegate.borrow().clone();

    // Step 3: drop the MacWindow Arc BEFORE invoking — defense in depth.
    drop(window);

    // Step 4: upgrade dyn Weak and invoke.
    if let Some(weak) = weak
        && let Some(strong) = weak.upgrade()
    {
        f(&*strong);
    }
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
