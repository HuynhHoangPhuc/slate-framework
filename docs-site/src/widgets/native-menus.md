# Native menus

Slate drives **real OS menus** from one platform-agnostic model: an `NSMenu`
bar on macOS and an `HMENU` on Windows. The same `Menu` / `MenuAction` model
produces the app menu bar and native context menus; selections route back into
the reactive runtime via handlers keyed by a stable `MenuId`.

> This is distinct from the in-canvas overlay
> [`ContextMenu`](overlay-widgets.md): native menus carry app-level commands and
> OS chrome; the in-canvas menu carries affordances bound to specific on-canvas
> content. The dashboard demonstrates both coexisting.

## Building a menu model

```rust
use slate_framework::{Accelerator, Key, Menu, MenuAction, MenuId, Modifiers};

const ID_INCREMENT: u64 = 1;
const ID_RESET: u64 = 2;
const ID_BOLD: u64 = 3;

fn cmd(ch: &str) -> Accelerator {
    Accelerator::new(
        Modifiers { meta: true, ..Default::default() },
        Key::Character(ch.into()),
    )
}

fn menu_bar(bold: bool) -> Menu {
    let file = Menu::new()
        .action(MenuAction::new(MenuId(ID_INCREMENT), "Increment").accelerator(cmd("i")))
        .action(MenuAction::new(MenuId(ID_RESET), "Reset"));
    let edit = Menu::new()
        .action(MenuAction::new(MenuId(ID_BOLD), "Bold").accelerator(cmd("b")).checked(bold));
    Menu::new().submenu("File", file).submenu("Edit", edit)
}
```

> Source: [`examples/native-menu/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/native-menu/src/main.rs).

## Installing & routing actions

Register handlers by `MenuId` and install the bar inside `App::run`. The handler
registry is install-independent, so it persists across menu swaps:

```rust
// Mutate signals from menu selections:
let c = count.clone();
cx.on_menu_action(MenuId(ID_INCREMENT), move || c.update(|n| *n += 1));

// A checked item: flip, then re-install the bar so the check mark reflects state.
let b = bold.clone();
let cx_bold = cx.clone();
cx.on_menu_action(MenuId(ID_BOLD), move || {
    b.update(|v| *v = !*v);
    cx_bold.set_menu(menu_bar(b.get()));
});

cx.set_menu(menu_bar(bold.get()));
```

> Source: [`examples/native-menu/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/native-menu/src/main.rs).

## Native context menus

Pop a native context menu at a point (e.g. from a key handler):

```rust
fn context_menu() -> Menu {
    Menu::new()
        .action(MenuAction::new(MenuId(ID_INCREMENT), "Increment"))
        .action(MenuAction::new(MenuId(ID_RESET), "Reset"))
}

// At a logical view point:
cx.show_context_menu(ctx.window_id(), context_menu(), (40.0, 60.0));
```

> Source: [`examples/native-menu/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/native-menu/src/main.rs).

## Key API

| Item | Purpose |
|---|---|
| `Menu::new()` | Build a menu (bar or popup). |
| `.submenu(title, Menu)` | Nest a submenu (top-level entries on a bar). |
| `.action(MenuAction)` / `.separator()` | Add an item / divider. |
| `MenuAction::new(MenuId, label)` | A command item. |
| `.accelerator(Accelerator)` | Key equivalent (`meta` maps to ⌘ / Ctrl). |
| `.checked(bool)` | Check-mark state. |
| `AppContext::set_menu(Menu)` | Install the app menu bar. |
| `AppContext::set_window_menu(WindowId, Menu)` | Per-window menu bar. |
| `AppContext::on_menu_action(MenuId, Fn())` | Register a handler (persists across swaps). |
| `AppContext::show_context_menu(WindowId, Menu, (f32, f32))` | Native context menu at a point. |

## Platform notes

- **macOS** synthesizes the standard **App ▸ Quit (⌘Q)** item itself; the focused
  window's menu is swapped into `NSApp.mainMenu` on focus.
- **Windows** keeps a true per-`HWND` menu bar; checked/grayed items and
  tab-aligned accelerator text are rendered natively.
- Selections are dispatched via a posted event to avoid re-entrancy inside the
  native modal menu loop.

## Accessibility

Native menus are owned by the OS, so the platform's own accessibility (VoiceOver
/ Narrator) announces and operates them — no Slate a11y node is emitted for the
native menu itself.
