# Button & IconButton

**`Button`** is a labelled, clickable command control. **`IconButton`** is its
icon-only sibling for compact toolbars; it requires an accessible label.

## Usage

```rust
use slate_framework::{Button, IconButton, Text, theme};

// A text button with a click handler:
Button::new("Save").on_click(|_| log::info!("Save clicked"))

// A disabled button:
Button::new("Disabled").disabled(true)

// An icon-only button — the second argument is the mandatory accessible name:
IconButton::new(
    Text::new("✕").font_size(theme().typography.body.size).color(theme().bg.into()).presentational(),
    "Dismiss",
)
```

> Source: [`examples/form-controls/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/form-controls/src/main.rs).

`IconButton` also accepts a plain glyph string and a chained handler, as used in
the dashboard toolbar:

```rust
IconButton::new("⟳", "Reload process list").on_click(|_| log::info!("Reload"))
```

> Source: [`examples/dashboard/src/panels.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/panels.rs).

## Key props

| Builder | Effect |
|---|---|
| `Button::new(label)` | Construct with a text label. |
| `IconButton::new(icon, accessible_label)` | Icon content + mandatory aria-label. |
| `.on_click(closure)` | Click / activation handler. |
| `.disabled(bool)` | Mark non-interactive (announced as disabled). |

Fills shade through interaction states (hover/active/disabled) and draw from
theme tokens automatically.

## Accessibility

- **Role:** `Button`.
- **Icon-only:** must carry a label — `IconButton`'s second argument supplies it.
- **Keyboard:** focusable; **Space / Enter** activates. A screen reader's default
  press routes through the same activation path, so the button is operable from
  the reader, not just announceable.

Mark decorative glyph/text inside a button with `Text::presentational()` to avoid
double-announcing the name.
