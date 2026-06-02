# Install & first window

## Requirements

- **Rust 1.95+** (the workspace pins the toolchain via `rust-toolchain.toml`).
- macOS or Windows 11. The GPU backend is `wgpu`; no extra system libraries are
  needed beyond a working graphics driver.

## Add the dependency

Slate's public API is re-exported from the umbrella crate `slate-framework`.
Add it to your `Cargo.toml`:

```toml
[dependencies]
slate-framework = { git = "https://github.com/HuynhHoangPhuc/slate-framework" }
```

Everything you import comes from the `slate_framework` crate root
(`use slate_framework::{...}`).

## A minimal window + element

A Slate app is an [`App`] configured with [`WindowOptions`], driven by a
[`View`] whose `render` returns an element tree. The smallest meaningful app
draws a styled [`Div`] with a [`Text`] child:

```rust
use slate_framework::{
    AlignItems, AnyElement, App, Color, Div, FlexDirection, IntoAny,
    JustifyContent, Text, View, WindowOptions,
};

struct HelloView;

impl View for HelloView {
    fn render(&mut self, _cx: &mut slate_framework::RenderCx) -> AnyElement {
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
            .child(Text::new("Hello, Slate!").font_size(32.0).color(Color::WHITE.into()))
            .into_any()
    }
}

fn main() {
    env_logger::init();

    App::new(WindowOptions {
        title: "Slate · hello-element".into(),
        size: (600, 400),
        min_size: Some((320, 240)),
        resizable: true,
        ..Default::default()
    })
    .run(|_cx| HelloView);
}
```

> Source: [`examples/hello-element/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/hello-element/src/main.rs).

### What's happening

- `App::new(WindowOptions { .. })` configures the first window. `WindowOptions`
  is **not** `#[non_exhaustive]`, so always finish the literal with
  `..Default::default()` — that keeps your code forward-compatible as fields are
  added.
- `.run(closure)` blocks, opens the window, and pumps the event loop. The
  closure receives an [`AppContext`] (`&AppContext`) and returns your `View`.
- `Div` is the base layout element; `Text` renders a string. `.into_any()`
  erases the concrete element type into an [`AnyElement`] for the return.
- Styling flows through the `.style(|s| ...)` builder (layout props) plus direct
  visual builders like `.background(..)` and `.corner_radius(..)`.

## Reacting to state

For anything interactive, hold state in a [`Signal`] created from the runtime
(`cx.runtime()`), and read it inside `render`. See
[Reactive signals](../concepts/reactive-signals.md) for the model, and
[`examples/reactive-counter`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/reactive-counter/src/main.rs)
for a focusable, keyboard-driven counter.

## Next steps

- [Running on macOS & Windows](running-on-macos-and-windows.md)
- [Reactive signals](../concepts/reactive-signals.md)
- [Widget reference](../widgets/README.md)

[`App`]: ../widgets/README.md
[`View`]: ../concepts/elements-vs-widgets.md
[`Div`]: ../concepts/elements-vs-widgets.md
[`Text`]: ../concepts/elements-vs-widgets.md
[`Signal`]: ../concepts/reactive-signals.md
[`AppContext`]: ../concepts/reactive-signals.md
[`AnyElement`]: ../concepts/elements-vs-widgets.md
[`WindowOptions`]: ../widgets/README.md
