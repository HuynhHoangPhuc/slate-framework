//! native-menu — P6b macOS native menu backend demo.
//!
//! Shows the platform-agnostic [`Menu`] model driving a real macOS menu bar
//! (`NSApp.mainMenu`) and a native context menu, with selections routed back
//! into the reactive runtime via `on_menu_action`:
//!
//! - **File ▸ Increment (⌘I)** — bumps the counter (also reachable as a menu
//!   key-equivalent, proving accelerator wiring).
//! - **File ▸ Reset** — sets the counter to 0.
//! - **Edit ▸ Bold (⌘B)** — toggles a checked item; the handler re-installs the
//!   menu so the check mark reflects state (enabled/checked reflection).
//! - **App ▸ Quit (⌘Q)** — synthesized standard app menu; clean shutdown.
//! - **Press `M`** — pops up a native context menu (Increment / Reset) at a
//!   fixed point, demonstrating `show_context_menu`.
//!
//! Run: `cargo run -p native-menu`

use std::cell::RefCell;
use std::rc::Rc;

use slate_framework::reactive::Signal;
use slate_framework::{
    Accelerator, AlignItems, AnyElement, App, AppContext, Color, Div, FlexDirection, IntoAny,
    JustifyContent, Key, Menu, MenuAction, MenuId, Modifiers, Text, View, WindowOptions,
};

// Stable command ids shared by the menu bar and the context menu.
const ID_INCREMENT: u64 = 1;
const ID_RESET: u64 = 2;
const ID_BOLD: u64 = 3;

/// Cmd+`ch` accelerator helper.
fn cmd(ch: &str) -> Accelerator {
    Accelerator::new(
        Modifiers {
            meta: true,
            ..Default::default()
        },
        Key::Character(ch.into()),
    )
}

/// Build the menu-bar model. `bold` drives the Edit ▸ Bold check mark.
fn menu_bar(bold: bool) -> Menu {
    let file = Menu::new()
        .action(MenuAction::new(MenuId(ID_INCREMENT), "Increment").accelerator(cmd("i")))
        .action(MenuAction::new(MenuId(ID_RESET), "Reset"));
    let edit = Menu::new()
        .action(MenuAction::new(MenuId(ID_BOLD), "Bold").accelerator(cmd("b")).checked(bold));
    Menu::new().submenu("File", file).submenu("Edit", edit)
}

/// The right-click / `M`-key context menu (shares command ids with the bar).
fn context_menu() -> Menu {
    Menu::new()
        .action(MenuAction::new(MenuId(ID_INCREMENT), "Increment"))
        .action(MenuAction::new(MenuId(ID_RESET), "Reset"))
}

struct MenuDemoView {
    count: Signal<i32>,
    bold: Signal<bool>,
}

impl View for MenuDemoView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        let count = self.count.clone();
        let bold = self.bold.clone();

        Div::new()
            .background(Color::from_hex("#1e1e2e").unwrap_or(Color::BLACK))
            .style(|s| {
                s.flex_direction(FlexDirection::Column)
                    .align_items(AlignItems::Center)
                    .justify_content(JustifyContent::Center)
                    .gap(16.0)
                    .padding_all(32.0)
                    .flex_grow(1.0)
            })
            .child(
                Text::new("Native Menu Demo")
                    .font_size(22.0)
                    .color(Color::WHITE.into()),
            )
            .child(
                Text::new_reactive(move || format!("count = {}", count.get()))
                    .font_size(44.0)
                    .color(Color::WHITE.into()),
            )
            .child(
                Text::new_reactive(move || {
                    format!("bold = {}", if bold.get() { "on" } else { "off" })
                })
                .font_size(16.0)
                .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .child(
                Text::new("File ▸ Increment (⌘I) · Reset   Edit ▸ Bold (⌘B)   Press M for context menu")
                    .font_size(12.0)
                    .color(Color::from_hex("#94a3b8").unwrap_or(Color::WHITE).into()),
            )
            .into_any()
    }
}

fn main() {
    env_logger::init();

    // Shared slot so the App-level key handler can reach the live `AppContext`
    // (allocated inside `App::run`). Same pattern as `two-window-counters`.
    let cx_handle: Rc<RefCell<Option<AppContext>>> = Rc::new(RefCell::new(None));
    let cx_for_key = cx_handle.clone();

    let opts = WindowOptions {
        title: "Slate · native menu".into(),
        size: (440, 300),
        min_size: Some((360, 240)),
        resizable: true,
        ..Default::default()
    };

    App::new(opts)
        .on_key_down(move |key, ctx| {
            // `M` (no modifiers) pops up the native context menu.
            if matches!(&key.key, Key::Character(c) if c.eq_ignore_ascii_case("m"))
                && !key.modifiers.meta
                && !key.modifiers.ctrl
                && let Some(cx) = cx_for_key.borrow().as_ref()
            {
                cx.show_context_menu(ctx.window_id(), context_menu(), (40.0, 60.0));
            }
        })
        .run(move |cx: &AppContext| {
            let count: Signal<i32> = Signal::new(cx.runtime(), 0);
            let bold: Signal<bool> = Signal::new(cx.runtime(), false);
            *cx_handle.borrow_mut() = Some(cx.clone());

            // Increment / Reset: mutate the counter signal.
            let c = count.clone();
            cx.on_menu_action(MenuId(ID_INCREMENT), move || c.update(|n| *n += 1));
            let c = count.clone();
            cx.on_menu_action(MenuId(ID_RESET), move || c.set(0));

            // Bold: toggle, then re-install the menu so the check mark reflects
            // the new state (enabled/checked reflection).
            let b = bold.clone();
            let cx_bold = cx.clone();
            cx.on_menu_action(MenuId(ID_BOLD), move || {
                b.update(|v| *v = !*v);
                cx_bold.set_menu(menu_bar(b.get()));
            });

            cx.set_menu(menu_bar(bold.get()));

            MenuDemoView { count, bold }
        });
}
