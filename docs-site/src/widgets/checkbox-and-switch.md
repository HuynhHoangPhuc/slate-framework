# Checkbox & Switch

Both are boolean toggles bound to a caller-owned `Signal<bool>`. `Checkbox` is
the check-mark form; `Switch` is the sliding-toggle form. They are
interchangeable in API.

## Usage

```rust
use slate_framework::{Checkbox, Switch};

// Each takes a Signal<bool> you own + a label:
Checkbox::new(self.notifications.clone(), "Enable notifications")
Switch::new(self.compact.clone(), "Compact layout")
```

> Source: [`examples/form-controls/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/form-controls/src/main.rs).

The signal is created once on your `View` so it survives the Strategy-A rebuild:

```rust
struct FormControlsView {
    notifications: Signal<bool>,
    compact: Signal<bool>,
    // ...
}
```

## Key props

| Builder | Effect |
|---|---|
| `Checkbox::new(signal, label)` | Bind a `Signal<bool>` + visible label. |
| `Switch::new(signal, label)` | Same, rendered as a switch. |

Toggling flips the bound signal, which triggers a whole-view rebuild; the
control re-reads the signal and repaints in the new state.

## Accessibility

- **Role:** `Checkbox` (Checkbox) / `Switch` (Switch).
- **State:** the on/off value is reported via `is_selected` — a reader announces
  checked / unchecked.
- **Keyboard:** focusable; **Space** toggles. The screen-reader press routes
  through the same toggle path, so the control is operable from the reader.
