//! `MenuTarget` — the retained Obj-C responder that receives `NSMenuItem`
//! target-action selections and routes them back into the framework.
//!
//! A single app-global instance owns the action target for every menu item
//! built by [`super::build`]. Items carry the framework `MenuId` in their
//! `tag`; on activation the responder reads the tag and posts
//! [`Event::MenuActivated`] through the same thread-local dispatch funnel every
//! other AppKit callback uses. The responder holds **no** strong reference to
//! framework state (it only posts an event), so it can be retained for the
//! app's lifetime without forming an `Arc` cycle.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType, NSMenuItem};
use objc2_foundation::{MainThreadMarker, NSPoint};

use super::super::{ffi_boundary, post_menu_event};
use crate::WindowId;

/// Instance state for [`MenuTarget`]: the window credited as the menu's owner
/// at activation time. Updated by `set_menu` / `show_context_menu` before the
/// menu is shown (last-writer-wins — multi-window menu ownership is a later
/// polish phase).
pub struct MenuTargetIvars {
    pub(crate) window: Cell<u64>,
}

define_class!(
    // SAFETY: NSObject has no subclassing restrictions; `MenuTarget` only adds
    // an interior-mutable `Cell<u64>` and posts events on the main thread.
    #[unsafe(super(objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = MenuTargetIvars]
    pub struct MenuTarget;

    // SAFETY: no additional protocol requirements.
    unsafe impl NSObjectProtocol for MenuTarget {}

    impl MenuTarget {
        /// Target-action for every actionable item. Reads the item's `tag`
        /// (the framework `MenuId`) and posts a deferred menu activation.
        ///
        /// Deferral (not an inline dispatch) is required: a context menu fires
        /// this inside its nested modal loop while the handler that opened it
        /// still holds the dispatch borrow. See [`post_menu_event`].
        #[unsafe(method(slateMenuAction:))]
        fn slate_menu_action(&self, sender: &NSMenuItem) {
            let id = sender.tag() as u64;
            let window = WindowId(self.ivars().window.get());
            ffi_boundary(|| {
                post_menu_event(window, id);
            });
        }

        /// Target-action for the synthesized standard "Quit" item. Mirrors
        /// [`MacPlatform::quit`](crate::mac::MacPlatform): `stop:` (so
        /// `app.run()` returns and `Event::Exiting` still fires) plus a
        /// synthetic wake so the loop exits immediately. Never `terminate:`,
        /// which `exit()`s and skips clean shutdown.
        #[unsafe(method(slateQuit:))]
        fn slate_quit(&self, _sender: &NSMenuItem) {
            ffi_boundary(|| {
                let mtm = MainThreadMarker::new()
                    .expect("slateQuit: runs on the AppKit main thread");
                let app = NSApplication::sharedApplication(mtm);
                app.stop(None);
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
                    app.postEvent_atStart(&wake, true);
                }
            });
        }
    }
);

impl MenuTarget {
    /// Allocate a new target with no owning window yet.
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(MenuTargetIvars {
            window: Cell::new(0),
        });
        // SAFETY: `NSObject`'s `init` has no additional requirements.
        unsafe { msg_send![super(this), init] }
    }

    /// Record the window credited as the owner of the next activation.
    pub(crate) fn set_window(&self, window: WindowId) {
        self.ivars().window.set(window.0);
    }
}
