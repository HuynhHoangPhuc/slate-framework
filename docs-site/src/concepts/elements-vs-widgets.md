# Elements vs widgets

Slate has two related concepts that are easy to conflate at first.

## Elements

An **element** is anything that implements the `Element` trait — the unit the
framework lays out and paints. Elements follow a **three-phase lifecycle**:
layout, prepaint, paint. The two primitives you use constantly are:

- **`Div`** — the base container: flexbox layout, background, corner radius,
  borders, interaction-state styling, focus, and event handlers.
- **`Text`** — a rendered string with font size and color, optionally reactive
  (`Text::new_reactive`) or `presentational()` (no accessibility node).

A `View` returns an element tree from `render`, type-erased to `AnyElement` via
`.into_any()`:

```rust
impl View for HelloView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
        Div::new()
            .child(Text::new("Hello, Slate!").font_size(32.0).color(Color::WHITE.into()))
            .into_any()
    }
}
```

> Source: [`examples/hello-element/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/hello-element/src/main.rs).

`Div` composes by chaining `.child(..)`, `.children(..)`, or `.child_any(..)`,
and styles via `.style(|s| ...)` for layout plus direct visual builders.

## Widgets

A **widget** is a higher-level, ready-made element built on the same trait —
`Button`, `Checkbox`, `Slider`, `Select`, `DataGrid`, and so on. Some widgets
are direct `Element` impls; others are `Div` compositions. From your code they
are all just elements you add as children:

```rust
Div::new()
    .child(Button::new("Save").on_click(|_| log::info!("Save clicked")))
    .child(Checkbox::new(notifications.clone(), "Enable notifications"))
    .child(Slider::new(volume.clone(), 0.0, 100.0).step(5.0))
```

> Source: [`examples/form-controls/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/form-controls/src/main.rs).

### Key distinction

- **Elements** (`Div`, `Text`) are the primitives you build *with*.
- **Widgets** are the batteries-included controls you build *from* — each carries
  its own interaction logic, theming, and accessibility role.

Both are the same trait under the hood, so they nest and compose freely: widgets
take `Div`/`Text` children, and your `Div`s hold widgets.

## The `IntoAny` / `AnyElement` boundary

`render` returns `AnyElement`. Concrete elements implement `IntoAny`, so you
finish a tree with `.into_any()`. Builders that accept arbitrary children take
`impl IntoAny` and call `.child_any(body.into_any())` internally — that is how
container widgets like `Panel` accept any element as their body.

See the [widget reference](../widgets/README.md) for every shipped widget.
