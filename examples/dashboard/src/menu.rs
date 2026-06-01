//! Native menu bar + action routing for the dashboard (P6f).
//!
//! The platform-agnostic [`Menu`] model drives a real OS menu bar
//! (`NSApp.mainMenu` on macOS, an `HMENU` on Windows) whose selections flip the
//! **same caller-owned signals** the in-canvas widgets already read — so a menu
//! command and the equivalent on-canvas control are one source of truth. This is
//! the desktop-native proof for the reference app.
//!
//! Native vs in-canvas (decision #1, both coexist): the native bar and the
//! native context menu carry **app-level commands / OS chrome**; the in-canvas
//! overlay [`ContextMenu`](slate_framework::ContextMenu) in `main_pane` carries
//! **affordances bound to specific on-canvas content** (the right-clicked row).

use slate_framework::reactive::Signal;
use slate_framework::{Accelerator, AppContext, Key, Menu, MenuAction, MenuId, Modifiers, ThemeMode};

use crate::DashboardView;

// Stable command ids shared by the menu bar and the native context menu.
const ID_REFRESH: u64 = 1;
const ID_COPY_PID: u64 = 2;
const ID_TOGGLE_THEME: u64 = 3;
const ID_SHOW_SYSTEM: u64 = 4;
const ID_AUTO_REFRESH: u64 = 5;

/// ⌘`ch` accelerator helper (the platform bridge maps `meta` to ⌘/Ctrl).
fn cmd(ch: &str) -> Accelerator {
    Accelerator::new(
        Modifiers {
            meta: true,
            ..Default::default()
        },
        Key::Character(ch.into()),
    )
}

/// Build the menu-bar model. The two View toggles reflect live signal state via
/// `.checked(...)`; macOS adds the App ▸ Quit (⌘Q) item itself.
fn menu_bar(show_system: bool, auto_refresh: bool) -> Menu {
    let file =
        Menu::new().action(MenuAction::new(MenuId(ID_REFRESH), "Refresh").accelerator(cmd("r")));
    let edit =
        Menu::new().action(MenuAction::new(MenuId(ID_COPY_PID), "Copy PID").accelerator(cmd("c")));
    let view = Menu::new()
        .action(MenuAction::new(MenuId(ID_TOGGLE_THEME), "Toggle Theme").accelerator(cmd("d")))
        .separator()
        .action(MenuAction::new(MenuId(ID_SHOW_SYSTEM), "Show System Processes").checked(show_system))
        .action(MenuAction::new(MenuId(ID_AUTO_REFRESH), "Auto-refresh").checked(auto_refresh));
    Menu::new().submenu("File", file).submenu("Edit", edit).submenu("View", view)
}

/// The native context menu (shares command ids with the bar, so its selections
/// route through the same handlers). Distinct from the in-canvas overlay menu.
pub(crate) fn native_context_menu() -> Menu {
    Menu::new()
        .action(MenuAction::new(MenuId(ID_REFRESH), "Refresh"))
        .action(MenuAction::new(MenuId(ID_COPY_PID), "Copy PID"))
}

/// Flip light/dark on the shared mode signal (identical to the toolbar button).
fn toggle_mode(mode: &Signal<ThemeMode>) {
    mode.update(|m| {
        *m = match *m {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        }
    });
}

/// Register every menu-action handler and install the bar. Called once in
/// `App::run`. Handlers persist across menu swaps (registry is install-
/// independent), so the checked-item handlers can re-install the bar to update
/// their check marks without re-registering.
pub(crate) fn install(cx: &AppContext, view: &DashboardView) {
    // Refresh / Copy PID — surface the action in the status bar.
    let last = view.last_action.clone();
    cx.on_menu_action(MenuId(ID_REFRESH), move || last.set("Refreshed process list".into()));
    let last = view.last_action.clone();
    cx.on_menu_action(MenuId(ID_COPY_PID), move || last.set("Copied PID to clipboard".into()));

    // Toggle theme — same signal the toolbar button drives.
    let mode = view.mode.clone();
    cx.on_menu_action(MenuId(ID_TOGGLE_THEME), move || toggle_mode(&mode));

    // Checked items — flip the signal, then re-install the bar so the check mark
    // reflects the new state.
    bind_toggle(cx, ID_SHOW_SYSTEM, &view.show_system, &view.auto_refresh);
    bind_toggle(cx, ID_AUTO_REFRESH, &view.auto_refresh, &view.show_system);

    cx.set_menu(menu_bar(view.show_system.get(), view.auto_refresh.get()));
}

/// Wire a checked-item handler: toggle `flag`, then rebuild the bar from both
/// toggle signals so the check marks stay in sync. `other` is the sibling
/// toggle whose current value the rebuild must preserve.
fn bind_toggle(cx: &AppContext, id: u64, flag: &Signal<bool>, other: &Signal<bool>) {
    let flag = flag.clone();
    let other = other.clone();
    let cx = cx.clone();
    cx.clone().on_menu_action(MenuId(id), move || {
        flag.update(|v| *v = !*v);
        // Re-read both toggles and rebuild, ordering by command id so the bar
        // layout never depends on which toggle fired.
        let (show_system, auto_refresh) = if id == ID_SHOW_SYSTEM {
            (flag.get(), other.get())
        } else {
            (other.get(), flag.get())
        };
        cx.set_menu(menu_bar(show_system, auto_refresh));
    });
}
