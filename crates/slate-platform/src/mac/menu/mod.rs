//! macOS native menu backend: lowers a [`PlatformMenu`] into `NSMenu` for the
//! app menu bar (`NSApp.mainMenu`) and native context menus, and routes
//! target-action selections back to the framework via [`Event::MenuActivated`].
//!
//! Split by concern: [`build`] constructs the native tree, [`target`] is the
//! retained action responder. The responder is created lazily and stored in a
//! thread-local `Retained` (main-thread-only), because `NSMenuItem.target` is a
//! weak reference — something stable must keep the responder alive for the
//! app's lifetime. It holds no framework state, so there is no `Arc` cycle.

mod build;
mod target;

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2_app_kit::NSApplication;
use objc2_foundation::{MainThreadMarker, NSPoint};

use self::target::MenuTarget;
use super::window::MacWindow;
use crate::{PlatformMenu, Window, WindowId};

thread_local! {
    /// The app-global action responder, created on first menu install.
    static MENU_TARGET: RefCell<Option<Retained<MenuTarget>>> = const { RefCell::new(None) };
}

/// Get (or lazily create) the shared action responder.
fn menu_target(mtm: MainThreadMarker) -> Retained<MenuTarget> {
    MENU_TARGET.with(|cell| {
        let mut slot = cell.borrow_mut();
        slot.get_or_insert_with(|| MenuTarget::new(mtm)).clone()
    })
}

/// Install `model` as the application menu bar (`NSApp.mainMenu`).
///
/// macOS has a single app-global menu bar; `window` is recorded as the owner
/// credited on the next activation (last-writer-wins until per-window menu
/// ownership lands in a later polish phase).
pub(crate) fn set_menu_bar(window: WindowId, model: &PlatformMenu, mtm: MainThreadMarker) {
    let target = menu_target(mtm);
    target.set_window(window);
    let bar = build::build_menu_bar(model, &target, mtm);
    NSApplication::sharedApplication(mtm).setMainMenu(Some(&bar));
}

/// Pop up `model` as a native context menu at `at` (logical view points,
/// top-left origin) over `mac_window`'s content view.
pub(crate) fn pop_up_context_menu(
    mac_window: &MacWindow,
    model: &PlatformMenu,
    at: (f32, f32),
    mtm: MainThreadMarker,
) {
    let target = menu_target(mtm);
    target.set_window(mac_window.id());
    let menu = build::build_context_menu(model, &target, mtm);

    let view = mac_window.content_view();
    // NSView is bottom-left origin (Y-up); the framework passes top-left points.
    let height = view.bounds().size.height;
    let location = NSPoint::new(at.0 as f64, height - at.1 as f64);
    let _shown = menu.popUpMenuPositioningItem_atLocation_inView(None, location, Some(view));
}
