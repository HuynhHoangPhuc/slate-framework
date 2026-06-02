# Dashboard — the flagship reference app

[`examples/dashboard/`](https://github.com/HuynhHoangPhuc/slate-framework/tree/main/examples/dashboard)
is Slate's full reference application: a **process/resource monitor** assembled
entirely from the shipped widget set. It is the single best place to see every
widget working together in real code.

```bash
cargo run -p dashboard
```

## What it exercises

| Area | Widgets |
|---|---|
| Top chrome | `Toolbar` (+ `Button`, `IconButton`, `Tooltip`) |
| Main split | `Splitter` dividing sidebar / main |
| Sidebar | `Tree` (process hierarchy) + `VirtualList` (flat process list) inside a `Panel` |
| Main pane | `DataGrid` (virtualized process table), `BarChart` + `Sparkline` viz, a settings strip of `Checkbox` / `Switch` / `Slider` / `TextField` / `Select`, all inside a `Panel` with a right-click `ContextMenu` |
| Bottom chrome | `StatusBar` |
| OS chrome | a **native menu bar** (File / Edit / View) + a native context menu (Shift+F10) |
| Theming | live light/dark toggle via `theme()` + `Signal<ThemeMode>` |
| Persistence | window geometry restored across restarts via a `PersistenceStore` |

## How it's structured

The example is deliberately modularized so each concern is easy to read:

| File | Responsibility |
|---|---|
| [`src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/main.rs) | App setup, the `DashboardView` state struct (all caller-owned signals), the frame: `Toolbar` + `Splitter` + `StatusBar`. |
| [`src/panels.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/panels.rs) | Panel builders: toolbar, sidebar (Tree + VirtualList), main pane (DataGrid + viz + settings + ContextMenu), status bar. |
| [`src/data.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/data.rs) | Synthetic fixtures (process names, tree, grid columns/rows, chart series). |
| [`src/menu.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/menu.rs) | Native menu bar + context menu; routes selections to the **same** signals the on-canvas widgets read. |
| [`src/store.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/store.rs) | A file-backed `PersistenceStore` for window geometry. |

## Design lessons it demonstrates

- **One source of truth.** A menu command and its on-canvas control flip the same
  caller-owned signal — e.g. View ▸ Toggle Theme and the toolbar theme button
  both drive `Signal<ThemeMode>`.
- **Caller-owned state survives rebuilds.** Every signal lives on `DashboardView`,
  created once in `App::run`, so the Strategy-A whole-view rebuild never loses
  state.
- **Native vs in-canvas menus coexist.** Shift+F10 pops a *native* context menu
  (app-level commands); right-click pops the *in-canvas overlay* `ContextMenu`
  (content affordances).
- **Assembly, not invention.** Every widget in the dashboard is an extracted
  reusable widget — the app is composed, not bespoke.

Use the dashboard as a working template when wiring your own multi-widget screen.
