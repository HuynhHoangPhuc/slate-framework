# List & VirtualList

**`List`** is a vertical, keyboard-navigable column of selectable rows.
**`VirtualList`** has the same builder surface but renders only the rows inside
the viewport, so a 100k-row list costs the same as a visible handful while still
reporting the true total to assistive tech.

Both use a **roving-focus** model: the container is the tab stop, and arrow keys
move real focus to the neighbouring row so a screen reader follows.

## List

```rust
use slate_framework::List;

// `selected` is a caller-owned Signal<Option<usize>> created outside `render`.
List::new(["CPU", "Memory", "Disk"], selected.clone())
    .label("Resources")
    .on_activate(|i, _cx| log::info!("activated row {i}"))
```

> Source (doc example):
> [`crates/slate-framework/src/elements/list/mod.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/crates/slate-framework/src/elements/list/mod.rs).

## VirtualList

`VirtualList` additionally takes a scroll-offset signal and a fixed height:

```rust
use slate_framework::VirtualList;

VirtualList::new(
    data::PROCESSES.iter().copied(),
    self.list_selected.clone(),   // Signal<Option<usize>>
    self.list_offset.clone(),     // Signal<f32> scroll offset
)
.label("All processes")
.height(180.0)
.on_activate(|i, _| log::info!("process row {i} activated"))
```

> Source: [`examples/dashboard/src/panels.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/panels.rs).

## Key props

| Builder | Effect |
|---|---|
| `List::new(items, selected)` | Rows + caller-owned `Signal<Option<usize>>` selection. |
| `VirtualList::new(items, selected, offset)` | Adds a `Signal<f32>` scroll offset. |
| `.label(name)` | Accessible name for the container. |
| `.height(f32)` | **Required for `VirtualList`** — fixed viewport height enables windowing. |
| `.on_activate(Fn(usize, &mut EventCtx))` | Fired on Enter / row click, after selection is written. |

The committed selection is caller-owned (Strategy A); the roving "active" row is
transient per-mount state, initialised to the current selection.

## Accessibility

- **Role:** `List` → `ListItem` subtree.
- **Selection:** the selected row reports `is_selected`.
- **Keyboard:** the container is the tab stop; **arrow keys** rove focus across
  rows (real focus moves so a reader follows); **Enter** activates the focused
  row. `VirtualList` reports the **true total** `row_count` even though only the
  visible window is materialized.
