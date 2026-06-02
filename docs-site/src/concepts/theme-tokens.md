# Theme tokens

Slate ships a semantic **theme token** system with ambient light/dark switching.
Instead of hard-coding colors, you read tokens from the ambient `theme()`
accessor, and a single `Signal<ThemeMode>` flips the whole app between palettes.

## The `theme()` ambient accessor

`theme()` is a **free function** — it reads the active `Theme` installed during
render, with no `Theme` value passed in (no prop-drilling). Call it wherever you
build elements:

```rust
use slate_framework::theme;

fn swatch(label: &'static str, color: Color) -> Div {
    let t = theme();
    Div::new()
        .style(|s| s.column().align_items(AlignItems::Center).gap(t.spacing.xs))
        .child(Div::new().background(color).corner_radius(t.radius.sm)
            .style(|s| s.width(56.0).height(40.0)))
        .child(Text::new(label).font_size(t.typography.caption.size).color(t.muted.into()))
}
```

> Source: [`examples/theme-switch/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/theme-switch/src/main.rs).

## What's on a `Theme`

| Group | Tokens |
|---|---|
| Semantic colors | `bg`, `surface`, `fg`, `muted`, `accent`, `border`, `danger` |
| Spacing scale | `spacing.xs / sm / md / lg / xl` |
| Radius scale | `radius.sm / md / lg` |
| Typography | `typography.caption / body / heading` (each a `TypeStyle` with `.size`) |

All token scale structs are `#[non_exhaustive]`.

## Switching light/dark

The active mode is a `Signal<ThemeMode>` obtained from the context. Get it once,
hold it on your `View`, and toggle it from a handler — the flip re-renders the
whole view with the other palette (Strategy A):

```rust
// Obtain the app-global mode signal inside App::run:
let mode = cx.theme_mode().expect("theme_mode available inside App::run");

// Toggle it from a button:
fn toggle(mode: &Signal<ThemeMode>) {
    mode.update(|m| {
        *m = match *m {
            ThemeMode::Light => ThemeMode::Dark,
            ThemeMode::Dark => ThemeMode::Light,
        }
    });
}
```

> Source: [`examples/theme-switch/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/theme-switch/src/main.rs).

`AppContext::theme_mode()` returns `Option<Signal<ThemeMode>>` (it is `None` in
headless mode). Reading `theme()` inside `render` subscribes the view to the
mode, so a flip rebuilds with the new tokens automatically.

## Custom palettes

Install your own light/dark `Theme`s at app construction:

```rust
App::new(opts).theme(my_light_theme, my_dark_theme).run(..)
```

The default is `ThemeSet::default()`. Themes are process-wide in v1 (per-window
theming is deferred).
