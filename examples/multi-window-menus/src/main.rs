//! multi-window-menus — per-window native menus + window-management helpers.
//!
//! Builds on the multi-window plumbing (`create_window`) to show the polish
//! layer:
//!
//! - **Per-window menus** — each window owns a distinct native menu via
//!   `AppContext::set_window_menu`. Window A's bar drives counter A; window B's
//!   drives counter B. On macOS (single shared app menu bar) the focused
//!   window's menu is swapped in on focus; on Windows each window has its own
//!   `HMENU`.
//! - **A "Window" menu** listing every open window (`AppContext::window_ids`),
//!   each entry focusing that window (`AppContext::focus_window`).
//! - **Cascade placement** — `New Window ⌘N` spawns a window offset from the
//!   others via `AppContext::cascade_position`.
//!
//! Quit: macOS gets a synthesized App ▸ Quit (⌘Q); Win32 quits on last-window
//! close.
//!
//! Run: `cargo run -p multi-window-menus`

mod counter_view;

use std::cell::RefCell;
use std::rc::Rc;

use slate_framework::reactive::Signal;
use slate_framework::{
    App, AppContext, Key, Menu, MenuAction, MenuId, Modifiers, WindowId, WindowOptions,
};

use counter_view::CounterView;

// Shared command ids. Per-window counter commands are offset by `cmd_base` so
// each window's Increment/Reset route to its own counter; focus commands are
// derived from the target window's raw id.
const ID_NEW_WINDOW: u64 = 1;
const CMD_BASE_STEP: u64 = 100; // window i gets [i*100+1, i*100+2]
const FOCUS_BASE: u64 = 1_000_000;

/// Per-window bookkeeping the demo needs to (re)build menus.
struct WinInfo {
    id: WindowId,
    title: String,
    count: Signal<i32>,
    cmd_base: u64,
}

/// ⌘`ch` accelerator helper.
fn cmd(ch: &str) -> slate_framework::Accelerator {
    slate_framework::Accelerator::new(
        Modifiers {
            meta: true,
            ..Default::default()
        },
        Key::Character(ch.into()),
    )
}

/// The shared "Window" submenu: New Window + one focus entry per open window.
fn window_submenu(infos: &[WinInfo]) -> Menu {
    let mut m = Menu::new()
        .action(MenuAction::new(MenuId(ID_NEW_WINDOW), "New Window").accelerator(cmd("n")));
    m = m.separator();
    for w in infos {
        m = m.action(MenuAction::new(
            MenuId(FOCUS_BASE + w.id.0),
            format!("Focus {}", w.title),
        ));
    }
    m
}

/// Build `info`'s own menu bar: a window-specific command submenu + the shared
/// Window menu listing all open windows.
fn menu_for(info: &WinInfo, infos: &[WinInfo]) -> Menu {
    let commands = Menu::new()
        .action(MenuAction::new(MenuId(info.cmd_base + 1), "Increment").accelerator(cmd("i")))
        .action(MenuAction::new(MenuId(info.cmd_base + 2), "Reset"));
    Menu::new()
        .submenu(info.title.clone(), commands)
        .submenu("Window", window_submenu(infos))
}

/// Install each window's menu and (re)register all action handlers. Called on
/// startup and after every spawn so the Window menu's open-window list stays
/// current.
fn rebuild_menus(cx: &AppContext, shared: &Rc<RefCell<Vec<WinInfo>>>) {
    let infos = shared.borrow();

    // Counter commands — one Increment/Reset pair per window.
    for info in infos.iter() {
        let c = info.count.clone();
        cx.on_menu_action(MenuId(info.cmd_base + 1), move || c.update(|n| *n += 1));
        let c = info.count.clone();
        cx.on_menu_action(MenuId(info.cmd_base + 2), move || c.set(0));

        // Focus command — raise the target window.
        let cx_focus = cx.clone();
        let target = info.id;
        cx.on_menu_action(MenuId(FOCUS_BASE + info.id.0), move || {
            cx_focus.focus_window(target)
        });
    }

    // New Window — spawn a cascaded window, then rebuild so its entry appears.
    let cx_spawn = cx.clone();
    let shared_spawn = shared.clone();
    cx.on_menu_action(MenuId(ID_NEW_WINDOW), move || {
        spawn_window(&cx_spawn, &shared_spawn)
    });

    // Push each window's distinct menu.
    for info in infos.iter() {
        cx.set_window_menu(info.id, menu_for(info, &infos));
    }
}

/// Spawn a new counter window offset from the existing ones, register it, and
/// rebuild every window's menu so the Window list includes it.
fn spawn_window(cx: &AppContext, shared: &Rc<RefCell<Vec<WinInfo>>>) {
    let index = shared.borrow().len() as u64;
    let title = format!("Window {}", (b'A' + index as u8) as char);
    let pos = cx.cascade_position((120, 120), 40);
    let count = Signal::new(cx.runtime(), 0i32);

    let opts = WindowOptions {
        title: format!("Slate · {title}"),
        size: (340, 240),
        min_size: Some((260, 180)),
        resizable: true,
        position: Some(pos),
        ..Default::default()
    };

    let view_count = count.clone();
    let view_title = title.clone();
    let Some(id) = cx.create_window(opts, move |_inner| CounterView {
        title: view_title.clone(),
        count: view_count.clone(),
    }) else {
        log::warn!("create_window returned None — platform handle missing");
        return;
    };

    shared.borrow_mut().push(WinInfo {
        id,
        title,
        count,
        cmd_base: (index + 1) * CMD_BASE_STEP,
    });
    rebuild_menus(cx, shared);
}

fn main() {
    env_logger::init();

    let shared: Rc<RefCell<Vec<WinInfo>>> = Rc::new(RefCell::new(Vec::new()));
    let shared_run = shared.clone();

    let opts = WindowOptions {
        title: "Slate · Window A".into(),
        size: (340, 240),
        min_size: Some((260, 180)),
        resizable: true,
        ..Default::default()
    };

    App::new(opts).run(move |cx: &AppContext| {
        let count = Signal::new(cx.runtime(), 0i32);
        // The first window's id is the first allocated (raw 1) — but read it
        // back from the window list so the demo never hard-codes platform ids.
        let id = cx.window_ids().first().copied().unwrap_or(WindowId(1));
        shared_run.borrow_mut().push(WinInfo {
            id,
            title: "Window A".into(),
            count: count.clone(),
            cmd_base: CMD_BASE_STEP,
        });
        rebuild_menus(cx, &shared_run);

        CounterView {
            title: "Window A".into(),
            count,
        }
    });
}
