# Overlay widgets

Widgets that ride the [overlay layer](../concepts/overlays.md): the raw
**`Overlay`** element, the **`Select`** dropdown, the **`Tooltip`**, the in-canvas
**`ContextMenu`**, and the shared **`MenuList`** core. They all open a high-depth
layer anchored to a target, without the in-canvas menus dimming the app.

## Overlay (raw)

The primitive: an anchored, optionally-modal layer with dismissal. See the
[overlays concept page](../concepts/overlays.md) for the full model.

```rust
use slate_framework::{Overlay, Placement};

Overlay::new()
    .anchor(self.anchor.clone())     // Bounds or Signal<Bounds>
    .placement(Placement::bottom())
    .on_dismiss(move || open.set(false))
    .child(/* popover body */)
```

```rust
// A modal dialog: scrim + focus trap.
Overlay::new()
    .modal(true)
    .anchor(Bounds::from_origin_size(140.0, 90.0, 200.0, 0.0))
    .placement(Placement::bottom())
    .on_dismiss(move || dismiss.set(false))
    .child(/* dialog body */)
```

> Source: [`examples/overlay-popover/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/overlay-popover/src/main.rs).

| Builder | Effect |
|---|---|
| `.anchor(Bounds \| Signal<Bounds>)` | Position target (live tracking via `Signal`). |
| `.placement(Placement::..)` | Preferred side; flips automatically near edges. |
| `.modal(bool)` | Scrim + focus trap + input blocking. |
| `.scrim(bool)` | Keep modal behavior but skip the dimming paint (default `true`). |
| `.on_dismiss(closure)` | Fired on Esc / outside-click / scrim-click. |

**A11y:** a modal overlay maps to `Dialog`; focus is trapped and restored.

## Select

A dropdown bound to a `Signal<usize>` (chosen index), plus an open flag and a
tracked anchor rect.

```rust
use slate_framework::Select;

Select::new(
    self.priority.clone(),      // Signal<usize> selected index
    self.select_open.clone(),   // Signal<bool> open flag
    self.select_anchor.clone(), // Signal<Bounds> trigger rect
)
.options(["Lowest", "Low", "Normal", "High", "Highest", "Critical"])
.scroll(self.select_scroll.clone())   // Signal<f32> list scroll
```

> Source: [`examples/overlay-widgets/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/overlay-widgets/src/main.rs).

| Builder | Effect |
|---|---|
| `Select::new(selected, open, anchor)` | Index, open flag, trigger-rect signals. |
| `.options(items)` | Option labels. |
| `.scroll(Signal<f32>)` | Scroll offset for a tall option list. |
| `.placement(..)` | Preferred open direction. |

**A11y:** the **closed trigger** announces as a `Button` named by the current
selection (there is no dedicated combobox role). The open list is a `Menu` →
`MenuItem` with `is_selected`. **Keyboard:** open moves focus into the list,
**arrows** rove, **Enter** chooses, **Esc / click-away** dismiss.

## Tooltip

Wraps a target; a delayed label bubble appears on hover and hides on leave. It is
non-interactive but always exposes its label to assistive tech.

```rust
use slate_framework::Tooltip;

Tooltip::new(self.tip_hover.clone(), "Permanently deletes the item")
    .child(Button::new("Delete"))
```

> Source: [`examples/overlay-widgets/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/overlay-widgets/src/main.rs).

| Builder | Effect |
|---|---|
| `Tooltip::new(hover_since, label)` | `Signal<Option<Instant>>` hover-start + label. |
| `.child(target)` | The element the tooltip describes. |
| `.delay(Duration)` | Hover delay before showing. |

**A11y:** the target is a `Group` `described_by` a persistent `Tooltip` node, so
the label is always available to a reader even before the bubble shows.

## ContextMenu (in-canvas)

An in-canvas right-click menu opened at the pointer. Distinct from the
[native OS context menu](native-menus.md).

```rust
use slate_framework::ContextMenu;

let items = ["Rename", "Duplicate", "Delete"];
ContextMenu::new(self.ctx_open.clone(), self.ctx_anchor.clone())
    .items(items)
    .on_select(move |i| last.set(format!("Context action: {}", items[i])))
    .child(target)
```

> Source: [`examples/overlay-widgets/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/overlay-widgets/src/main.rs).

| Builder | Effect |
|---|---|
| `ContextMenu::new(open, anchor)` | `Signal<bool>` open + `Signal<Bounds>` pointer anchor. |
| `.items(items)` | Menu entries. |
| `.on_select(Fn(usize))` | Selection handler. |
| `.child(target)` | Right-click target that opens the menu. |

**A11y:** `Menu` → `MenuItem`. **Keyboard:** arrow-navigable; dismiss on choose /
Esc / outside-click.

## MenuList (shared core)

The keyboard-navigable list of entries that `Select` and `ContextMenu` are built
on. Use it directly to build custom menu surfaces:

```rust
use slate_framework::{MenuList, MenuEntry};

MenuList::new(items)
    .selected(Some(2))
    .initial_active(2)
    .on_activate(|i, _cx| log::info!("chose {i}"))
```

| Builder | Effect |
|---|---|
| `MenuList::new(items)` | Entry list (`MenuEntry { label, disabled }`). |
| `.selected(Option<usize>)` | Marked-selected entry. |
| `.initial_active(usize)` | Starting roving-focus row. |
| `.on_activate(Fn(usize, &mut EventCtx))` | Activation handler. |

**A11y:** `Menu` → `MenuItem` with roving focus.
