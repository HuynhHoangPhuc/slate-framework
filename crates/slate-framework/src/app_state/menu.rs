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
    ///
    /// This is the app-global default; per-window overrides installed via
    /// [`install_window_menu`](Self::install_window_menu) take precedence and
    /// are not clobbered here.
    pub(crate) fn install_menu(&self, menu: Menu) {
        let platform_menu = menu.to_platform();
        *self.active_menu.borrow_mut() = Some(menu);
        let overrides = self.window_menus.borrow();
        let guard = self.windows.borrow();
        for (id, win) in guard.iter() {
            // A window with its own menu keeps it; the global default only
            // applies to windows without an override.
            if !overrides.contains_key(id) {
                win.window.set_menu(&platform_menu);
            }
        }
    }

    /// Install `menu` as `window`'s per-window menu, overriding the app-global
    /// default for that window. Stores the model and pushes it to that window
    /// via the platform seam.
    ///
    /// On Windows each window owns its own `HMENU`, so this takes effect
    /// immediately. On macOS — a single shared app menu bar — the override is
    /// recorded and installed whenever the window becomes key (see
    /// [`handle_window_focused`](Self::handle_window_focused)); it is also
    /// installed here so a menu set on the currently-key window appears at once.
    pub(crate) fn install_window_menu(&self, window: WindowId, menu: Menu) {
        let platform_menu = menu.to_platform();
        self.window_menus.borrow_mut().insert(window, menu);
        let guard = self.windows.borrow();
        if let Some(win) = guard.get(&window) {
            win.window.set_menu(&platform_menu);
        }
    }

    /// The menu that should be shown for `window`: its per-window override if
    /// present, else the app-global default. `None` when neither is set.
    fn resolve_window_menu(&self, window: WindowId) -> Option<Menu> {
        self.window_menus
            .borrow()
            .get(&window)
            .cloned()
            .or_else(|| self.active_menu.borrow().clone())
    }

    /// Push `window`'s resolved menu (per-window override, else the app-global
    /// default) to that window via the platform seam. No-op for an unknown
    /// `window` or when no menu is set anywhere.
    ///
    /// Used both on focus (macOS bar swap) and right after a deferred-created
    /// window is materialised, so a window spawned via
    /// [`AppContext::create_window`](crate::AppContext::create_window) — whose
    /// `WindowState` is inserted after the spawning handler unwinds — still
    /// gets its menu deterministically on both platforms (rather than relying
    /// on a later focus notification).
    pub(crate) fn apply_window_menu(&self, window: WindowId) {
        if let Some(menu) = self.resolve_window_menu(window) {
            let platform_menu = menu.to_platform();
            let guard = self.windows.borrow();
            if let Some(win) = guard.get(&window) {
                win.window.set_menu(&platform_menu);
            }
        }
    }

    /// Install the focused window's resolved menu into the (shared) native menu
    /// bar. Driven by [`Event::WindowFocused`](slate_platform::Event); this is
    /// how per-window menu ownership works on macOS, where there is one app menu
    /// bar that must track the key window. On Windows the per-window `HMENU` is
    /// already attached, so re-installing here is a harmless idempotent refresh.
    pub(crate) fn handle_window_focused(&self, window: WindowId) -> AppSignal {
        self.apply_window_menu(window);
        AppSignal::None
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
            .resolve_window_menu(window)
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::rc::Rc;

    use slate_platform::WindowId;

    use crate::executor::{Executor, RedrawRequester};
    use crate::menu::{Menu, MenuAction, MenuId};

    use super::super::state::AppState;
    use super::super::types::AppSignal;

    fn empty_state() -> Rc<AppState> {
        let redraw = RedrawRequester::new(slate_platform::wake_run_loop);
        let executor = Executor::new(redraw.clone());
        let runtime = slate_reactive::Runtime::new();
        Rc::new(AppState::new(executor, redraw, runtime))
    }

    fn win(n: u64) -> WindowId {
        WindowId(n)
    }

    #[test]
    fn per_window_menu_overrides_global_for_enable_gate() {
        let state = empty_state();
        // Global menu enables id 1; window B's override disables it.
        state.install_menu(Menu::new().action(MenuAction::new(MenuId(1), "Go")));
        state.install_window_menu(
            win(2),
            Menu::new().action(MenuAction::new(MenuId(1), "Go").enabled(false)),
        );

        let hits = Rc::new(Cell::new(0u32));
        let h = hits.clone();
        state.register_menu_action(MenuId(1), move || h.set(h.get() + 1));

        // Window 1 (no override) uses the global menu → enabled → handler runs.
        assert!(matches!(
            state.dispatch_menu(win(1), MenuId(1)),
            AppSignal::RequestRedraw { .. }
        ));
        assert_eq!(hits.get(), 1);

        // Window 2 (override disables id 1) → gated → handler does NOT run.
        assert!(matches!(
            state.dispatch_menu(win(2), MenuId(1)),
            AppSignal::None
        ));
        assert_eq!(hits.get(), 1);
    }

    #[test]
    fn focus_handler_is_noop_without_a_menu() {
        let state = empty_state();
        // No menu installed anywhere, unknown window — must not panic.
        assert!(matches!(
            state.handle_window_focused(win(7)),
            AppSignal::None
        ));
    }

    #[test]
    fn destroy_drops_per_window_menu_override() {
        let state = empty_state();
        state.install_window_menu(
            win(3),
            Menu::new().action(MenuAction::new(MenuId(1), "X").enabled(false)),
        );
        assert!(state.window_menus.borrow().contains_key(&win(3)));
        state.handle_window_destroyed(win(3));
        assert!(!state.window_menus.borrow().contains_key(&win(3)));
    }
}
