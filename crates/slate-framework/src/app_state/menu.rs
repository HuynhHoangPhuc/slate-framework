//! Native-menu installation + action routing on `AppState`.
//!
//! Framework-side half of P6 native menus: store the active [`Menu`] model,
//! register action handlers keyed by [`MenuId`], and dispatch an activation id
//! to its handler. The platform `set_menu` seam is invoked with a lowered
//! [`PlatformMenu`]; with no native backend it is a no-op (P6b/P6c make it
//! real), so the whole path is testable without FFI.

use slate_platform::{Window, WindowId};

use crate::menu::{Menu, MenuId};

use super::state::AppState;
use super::types::AppSignal;

impl AppState {
    /// Register (or replace) the handler invoked when the menu item carrying
    /// `id` is activated. Handlers capture `Signal`/`AppContext` clones; the
    /// mutation they perform schedules the next view rebuild.
    pub(crate) fn register_menu_action(&self, id: MenuId, handler: impl Fn() + 'static) {
        self.menu_registry.borrow_mut().register(id, handler);
    }

    /// Install `menu` as the active native menu-bar model: store it, lower it to
    /// the platform descriptor, and push it to every window via the platform
    /// `set_menu` seam (no-op until a native backend lands).
    pub(crate) fn install_menu(&self, menu: Menu) {
        let platform_menu = menu.to_platform();
        *self.active_menu.borrow_mut() = Some(menu);
        let guard = self.windows.borrow();
        for win in guard.values() {
            win.window.set_menu(&platform_menu);
        }
    }

    /// Pop up `menu` as a native context menu over `window`, anchored at `at`
    /// (logical view points, top-left origin). Lowers the model and calls the
    /// platform seam; no-op until a native backend lands, and no-op for an
    /// unknown `window`. Context-menu items route through the same action
    /// registry as the menu bar, so register their handlers via
    /// [`register_menu_action`](Self::register_menu_action).
    pub(crate) fn show_context_menu(&self, window: WindowId, menu: Menu, at: (f32, f32)) {
        let platform_menu = menu.to_platform();
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.window.show_context_menu(&platform_menu, at);
        }
    }

    /// Route a menu activation `id` to its registered handler.
    ///
    /// Follows the deferred-op discipline: clone the handler out under a short
    /// borrow, drop the borrow, then invoke — so a handler that rebuilds the
    /// menu (mutating the registry) cannot alias a live borrow. Returns
    /// `RequestRedraw` when a handler ran (it may have mutated signals),
    /// `None` when no handler is registered for `id`.
    pub(crate) fn dispatch_menu(&self, window: WindowId, id: MenuId) -> AppSignal {
        // Defense in depth: native backends grey out disabled items so the OS
        // never delivers their activation, but a synthetic/buggy caller might.
        // Drop the borrow before touching the registry.
        let enabled = self
            .active_menu
            .borrow()
            .as_ref()
            .map(|m| m.is_enabled(id))
            .unwrap_or(true);
        if !enabled {
            return AppSignal::None;
        }
        let handler = self.menu_registry.borrow().handler(id);
        match handler {
            Some(h) => {
                h();
                AppSignal::RequestRedraw { window }
            }
            None => AppSignal::None,
        }
    }
}
