# Layout (Taffy flexbox)

Slate lays out elements with [Taffy](https://github.com/DioxusLabs/taffy), a
flexbox engine. You drive it through the `.style(|s| ...)` builder on `Div` (and
on widgets that expose styling). The mental model is CSS flexbox.

## The style builder

`.style(|s| ...)` receives a style object and returns it after chaining layout
properties:

```rust
Div::new().style(|s| {
    s.flex_direction(FlexDirection::Column)
        .align_items(AlignItems::Center)
        .justify_content(JustifyContent::Center)
        .gap(16.0)
        .padding_all(32.0)
        .flex_grow(1.0)
})
```

> Source: [`examples/hello-element/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/hello-element/src/main.rs).

## Common properties

| Property | Purpose |
|---|---|
| `flex_direction(FlexDirection::Row \| Column)` | Main-axis direction. |
| `align_items(AlignItems::..)` | Cross-axis alignment (`Center`, `Stretch`, `FlexEnd`, …). |
| `justify_content(JustifyContent::..)` | Main-axis distribution (`Center`, …). |
| `gap(f32)` | Space between children. |
| `padding_all(f32)` | Uniform padding. |
| `flex_grow(f32)` | Share of leftover main-axis space (use `1.0` on a root to fill the window). |
| `width(f32)` / `height(f32)` | Fixed pixel size. |

## Row/column shorthands

Many examples use the `.row()` / `.column()` shorthands instead of
`flex_direction(..)`:

```rust
Div::new().style(|s| s.row().gap(t.spacing.md))
Div::new().style(|s| s.column().gap(t.spacing.sm).padding_all(t.spacing.lg))
```

> Source: [`examples/overlay-widgets/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/overlay-widgets/src/main.rs).

## Filling the window

A root container typically sets `flex_grow(1.0)` so it expands to the full
window, then arranges children inside:

```rust
Div::new()
    .background(t.bg)
    .style(|s| s.column().gap(t.spacing.lg).padding_all(t.spacing.xl).flex_grow(1.0))
```

## Sizing units note

Sizes are **logical points** (DPI-independent). Fixed sizes are plain `f32`
pixels (`.width(120.0)`). Note that the virtualized widgets
(`VirtualList`, `ScrollView::virtualized`) require a **fixed pixel height** on
their viewport to window their rows — see
[List & VirtualList](../widgets/list-and-virtual-list.md).
