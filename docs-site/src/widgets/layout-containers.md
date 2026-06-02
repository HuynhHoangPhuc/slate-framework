# Layout containers

Four chrome-building widgets: **`Panel`** (titled card), **`Toolbar`**
(horizontal command strip), **`StatusBar`** (bottom status strip), and
**`Splitter`** (resizable two-pane container).

## Panel

A titled `surface` card with an optional scrollable body.

```rust
use slate_framework::Panel;

Panel::new("Processes").child(/* body element */)
```

> Source: [`examples/dashboard/src/panels.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/panels.rs).

| Builder | Effect |
|---|---|
| `Panel::new(title)` | Titled bordered card. |
| `.child(..)` | Body content. |
| `.scroll(Signal<f32>)` | Make the body scrollable with a caller-owned offset. |

**A11y:** `Group`, labelled by the title; the title text is presentational to
avoid double-announce.

## Toolbar

A horizontal strip hosting command controls, with optional separators.

```rust
use slate_framework::Toolbar;

Toolbar::new()
    .child(Button::new("Refresh").on_click(|_| log::info!("Refresh clicked")))
    .child(IconButton::new("⟳", "Reload process list").on_click(|_| log::info!("Reload")))
    .separator()
    .child(Button::new("End Process"))
```

> Source: [`examples/dashboard/src/panels.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/panels.rs).

| Builder | Effect |
|---|---|
| `Toolbar::new()` | Empty horizontal strip. |
| `.child(..)` | Add a control. |
| `.separator()` | Visual divider. |

**A11y:** unlabelled `Group`.

## StatusBar

A bottom strip of status segments (muted caption text, indicators, dividers).

```rust
use slate_framework::StatusBar;

StatusBar::new()
    .text(format!("{} processes", data::PROCESSES.len()))
    .separator()
    .text("CPU 12%")
    .separator()
    .text(self.last_action.get())
```

> Source: [`examples/dashboard/src/panels.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/panels.rs).

| Builder | Effect |
|---|---|
| `StatusBar::new()` | Empty status strip. |
| `.text(String)` | Muted caption segment. |
| `.child(..)` | Arbitrary indicator. |
| `.separator()` | Divider. |

**A11y:** unlabelled `Group`.

## Splitter

A two-pane resizable container bound to a caller-owned split-ratio signal (the
first pane's fraction).

```rust
use slate_framework::Splitter;

Splitter::new(self.split.clone())   // Signal<f32> first-pane fraction
    .first(self.sidebar())
    .second(self.main_pane())
    .min_sizes(160.0, 360.0)
    .label("Sidebar / main divider")
```

> Source: [`examples/dashboard/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/main.rs).

| Builder | Effect |
|---|---|
| `Splitter::new(Signal<f32>)` | Bind the split ratio (first-pane fraction). |
| `.first(..)` / `.second(..)` | Pane contents. |
| `.min_sizes(f32, f32)` | Per-pane minimum sizes. |
| `.label(name)` | Accessible name for the divider. |

**A11y:** the **divider** is a `Slider` reporting the split percentage and
exposing **Increment / Decrement**. **Keyboard:** focus the divider, then
**Arrow / Home / End** resize; it is also draggable by pointer.
