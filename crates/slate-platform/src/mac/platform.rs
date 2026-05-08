//! MacPlatform — macOS platform driver and application delegates.

use std::cell::Cell;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{ClassType, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSEvent,
    NSEventModifierFlags, NSEventType,
};
use objc2_foundation::{MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint};

use crate::{Event, Platform, WindowOptions};
use super::window::MacWindow;
use super::{dispatch_event, ffi_boundary, HANDLER, REDRAW_EVENT_SUBTYPE};
use crate::WindowId;

// ---------------------------------------------------------------------------
// SlateApplication — custom NSApplication subclass to intercept synthetic events
// ---------------------------------------------------------------------------

define_class!(
    #[unsafe(super(NSApplication, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    pub struct SlateApplication;

    unsafe impl NSObjectProtocol for SlateApplication {}

    impl SlateApplication {
        /// Override sendEvent: to intercept our synthetic redraw events before
        /// they reach the default (no-op) routing for ApplicationDefined events.
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            if event.r#type() == NSEventType::ApplicationDefined
                && event.subtype() == objc2_app_kit::NSEventSubtype(REDRAW_EVENT_SUBTYPE)
            {
                let window_id = WindowId(event.data1() as u64);
                ffi_boundary(|| {
                    dispatch_event(Event::WindowRedrawRequested { window: window_id });
                });
                return;
            }
            // Forward all other events to the default NSApplication handling.
            let _: () = unsafe { msg_send![super(self), sendEvent: event] };
        }
    }
);

impl SlateApplication {
    fn shared(_mtm: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::class(), sharedApplication] }
    }
}

// ---------------------------------------------------------------------------
// AppDelegate — application-level callbacks
// ---------------------------------------------------------------------------

define_class!(
    // SAFETY: NSObject has no subclassing restrictions; AppDelegate carries no
    // Rust state beyond the empty ivars tuple.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ()]
    pub struct AppDelegate;

    // SAFETY: no requirements.
    unsafe impl NSObjectProtocol for AppDelegate {}

    // SAFETY: NSApplicationDelegate has no additional safety requirements.
    unsafe impl NSApplicationDelegate for AppDelegate {
        /// Sent after the run loop starts and the app is ready.
        /// This is the canonical point to emit `Event::Resumed`.
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn application_did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = self.mtm();
            ffi_boundary(|| {
                // Bring the app to front when launched from `cargo run` (unbundled).
                #[allow(deprecated)]
                NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
                dispatch_event(Event::Resumed);
            });
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        // SAFETY: `NSObject`'s `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// MacPlatform — public platform handle
// ---------------------------------------------------------------------------

/// The macOS platform driver.
///
/// Must be created and used exclusively on the main OS thread.
pub struct MacPlatform {
    mtm: MainThreadMarker,
    app: Retained<NSApplication>,
    /// Guards against re-entrant calls to `run`.
    running: Cell<bool>,
}

impl Platform for MacPlatform {
    type Window = MacWindow;

    fn new() -> Self {
        // Panic immediately if not on the main thread — AppKit mandates it.
        let mtm = MainThreadMarker::new()
            .expect("MacPlatform::new must be called from the main OS thread");

        // Use our NSApplication subclass so sendEvent: intercepts redraw events.
        let slate_app = SlateApplication::shared(mtm);
        let app: Retained<NSApplication> = Retained::into_super(slate_app);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

        MacPlatform {
            mtm,
            app,
            running: Cell::new(false),
        }
    }

    fn create_window(&self, opts: WindowOptions) -> Arc<MacWindow> {
        MacWindow::new(&opts, self.mtm)
    }

    fn run<F>(&self, mut handler: F)
    where
        F: FnMut(Event),
    {
        assert!(!self.running.get(), "MacPlatform::run called re-entrantly");
        self.running.set(true);

        // Erase the handler's lifetime to 'static for HANDLER thread-local storage.
        //
        // SAFETY:
        //   (1) `NSApplication::run` blocks this call stack until `terminate:`
        //       or `stop:` is called — i.e. until the run loop exits.
        //   (2) All AppKit callbacks that access HANDLER execute on this same
        //       main thread (single-threaded UI); there is no concurrent access.
        //   (3) We unconditionally clear HANDLER before this function returns
        //       (in the `finally` section below), so the raw pointer embedded
        //       in the Box is never dereferenced after the closure's borrow ends.
        //   (4) Therefore the transmute is equivalent to lending a &'run_scope
        //       reference to a scope that is strictly nested within run_scope.
        let handler_static: Box<dyn FnMut(Event) + 'static> = {
            let erased: Box<dyn FnMut(Event) + '_> = Box::new(&mut handler);
            // SAFETY: see argument above.
            unsafe { std::mem::transmute(erased) }
        };

        HANDLER.with(|h| {
            *h.borrow_mut() = Some(handler_static);
        });

        // Install the app delegate (sends Resumed in applicationDidFinishLaunching).
        let delegate = AppDelegate::new(self.mtm);
        self.app
            .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        // Enter the native run loop. Blocks until terminate:/stop: is called.
        self.app.run();

        // --- run loop has exited; emit Exiting while handler is still live ---
        dispatch_event(Event::Exiting);

        // CRITICAL: clear the handler to invalidate the 'static-erased pointer
        // BEFORE this function returns and the borrow of `handler` ends.
        HANDLER.with(|h| {
            *h.borrow_mut() = None;
        });

        self.running.set(false);
    }

    fn quit(&self) {
        // Use `stop:` rather than `terminate:` so `app.run()` returns control
        // to our caller — `terminate:` invokes `exit()` and skips the
        // `Event::Exiting` dispatch in `run()`.
        //
        // `stop:` only takes effect on the next event-loop iteration; if no
        // events are pending the loop blocks indefinitely. Post a synthetic
        // ApplicationDefined event to guarantee the loop wakes immediately.
        self.app.stop(None);
        let event = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        );
        if let Some(event) = event {
            self.app.postEvent_atStart(&event, true);
        }
    }
}
