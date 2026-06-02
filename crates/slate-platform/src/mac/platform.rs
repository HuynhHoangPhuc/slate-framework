//! MacPlatform — macOS platform driver and application delegates.

use std::cell::Cell;
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{ClassType, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSEvent,
    NSEventModifierFlags, NSEventType, NSMenu,
};
use objc2_foundation::{MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint};

use super::window::MacWindow;
use super::{
    A11Y_ACTIVATE_SUBTYPE, A11Y_FOCUS_SUBTYPE, HANDLER, MENU_EVENT_SUBTYPE, REDRAW_EVENT_SUBTYPE,
    WAKE_EVENT_SUBTYPE, dispatch_event, ffi_boundary,
};
use crate::WindowId;
use crate::{Event, Platform, WindowOptions};

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
        /// Override sendEvent: to intercept our synthetic redraw and wake events before
        /// they reach the default (no-op) routing for ApplicationDefined events.
        ///
        /// Also intercepts Cmd+<key> KeyDown/KeyUp so they reach the view's
        /// `keyDown:` / `keyUp:` selectors instead of being absorbed by the
        /// main menu's key-equivalent matcher. The only carveout is Cmd+Q,
        /// which falls through to `super` so the system Quit menu item still
        /// terminates the app.
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            if event.r#type() == NSEventType::ApplicationDefined {
                let subtype = event.subtype();
                if subtype == objc2_app_kit::NSEventSubtype(REDRAW_EVENT_SUBTYPE) {
                    let window_id = WindowId(event.data1() as u64);
                    ffi_boundary(|| {
                        dispatch_event(Event::WindowRedrawRequested { window: window_id });
                    });
                    return;
                }
                if subtype == objc2_app_kit::NSEventSubtype(WAKE_EVENT_SUBTYPE) {
                    ffi_boundary(|| {
                        dispatch_event(Event::Wake);
                    });
                    return;
                }
                if subtype == objc2_app_kit::NSEventSubtype(MENU_EVENT_SUBTYPE) {
                    let window = WindowId(event.data1() as u64);
                    let id = event.data2() as u64;
                    ffi_boundary(|| {
                        dispatch_event(Event::MenuActivated { window, id });
                    });
                    return;
                }
                if subtype == objc2_app_kit::NSEventSubtype(A11Y_FOCUS_SUBTYPE)
                    || subtype == objc2_app_kit::NSEventSubtype(A11Y_ACTIVATE_SUBTYPE)
                {
                    let window = WindowId(event.data1() as u64);
                    let node = event.data2() as u64;
                    let action = if subtype == objc2_app_kit::NSEventSubtype(A11Y_FOCUS_SUBTYPE) {
                        crate::A11yAction::Focus
                    } else {
                        crate::A11yAction::Activate
                    };
                    ffi_boundary(|| {
                        dispatch_event(Event::AccessibilityAction {
                            window,
                            node,
                            action,
                        });
                    });
                    return;
                }
            }

            // Cmd+key routing: this app installs no main menu, so the two
            // system shortcuts that rely on a menu key-equivalent (Quit, the
            // emoji/character palette) are handled explicitly here, and every
            // other Cmd+combo is delivered straight to the first responder's
            // keyDown:/keyUp: (the framework's TextField/TextArea own
            // Cmd+C/V/X etc.).
            let ev_ty = event.r#type();
            if matches!(ev_ty, NSEventType::KeyDown | NSEventType::KeyUp) {
                let flags = event.modifierFlags();
                if flags.contains(NSEventModifierFlags::Command) {
                    // Give an installed main menu first crack at the key event.
                    // We override sendEvent: and route Cmd+combos straight to the
                    // first responder below, which bypasses AppKit's normal menu
                    // key-equivalent matching — so native menu accelerators would
                    // never fire without this. `performKeyEquivalent:` only claims
                    // keys the menu actually defines, so first-responder shortcuts
                    // the menu doesn't list (TextField Cmd+C/V/X) still fall
                    // through. No menu installed → `mainMenu` is nil → skipped.
                    if ev_ty == NSEventType::KeyDown
                        && let Some(menu) = self.mainMenu()
                        && NSMenu::performKeyEquivalent(&menu, event)
                    {
                        return;
                    }
                    let vk = event.keyCode();
                    // kVK_ANSI_Q = 0x0C, kVK_Space = 0x31.
                    let is_quit = vk == 0x0C;
                    let is_char_palette =
                        vk == 0x31 && flags.contains(NSEventModifierFlags::Control);

                    if ev_ty == NSEventType::KeyDown && is_quit {
                        // No menu Quit item exists to match Cmd+Q. Stop the run
                        // loop directly (mirrors MacPlatform::quit) so app.run()
                        // returns and Event::Exiting still fires — never
                        // terminate:, which exit()s and skips clean shutdown.
                        // stop: only takes effect on the next loop iteration, so
                        // post a synthetic event to wake the loop immediately.
                        self.stop(None);
                        let wake = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
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
                        if let Some(wake) = wake {
                            self.postEvent_atStart(&wake, true);
                        }
                        return;
                    }

                    if ev_ty == NSEventType::KeyDown && is_char_palette {
                        // Cmd+Ctrl+Space → system emoji & symbols picker. The
                        // first-responder routing below would otherwise swallow
                        // it before AppKit's character-palette action runs.
                        self.orderFrontCharacterPalette(None);
                        return;
                    }

                    // Swallow the key-up partners of the two shortcuts above so
                    // no stray event leaks to the responder.
                    if is_quit || is_char_palette {
                        return;
                    }

                    if let Some(window) = self.keyWindow()
                        && let Some(first_responder) = window.firstResponder()
                    {
                        // Send the key event directly to the first responder,
                        // skipping the main menu's performKeyEquivalent: path.
                        // SAFETY: NSResponder responds to keyDown:/keyUp: with
                        // a single NSEvent argument; both selectors are valid.
                        unsafe {
                            if ev_ty == NSEventType::KeyDown {
                                let _: () = msg_send![&*first_responder, keyDown: event];
                            } else {
                                let _: () = msg_send![&*first_responder, keyUp: event];
                            }
                        }
                        return;
                    }
                }
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
