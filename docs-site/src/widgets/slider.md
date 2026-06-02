# Slider

A horizontal value control bound to a caller-owned `Signal<f32>`, configured
with a min/max range and an optional step.

## Usage

```rust
use slate_framework::Slider;

Slider::new(self.volume.clone(), 0.0, 100.0)
    .step(5.0)
    .width(220.0)
    .label("Volume")
```

> Source: [`examples/form-controls/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/form-controls/src/main.rs).

Read the same signal elsewhere to show a live readout — the readout rebuilds
whenever the slider writes:

```rust
Text::new_reactive(move || format!("Volume: {}", vol.get() as i64))
```

## Key props

| Builder | Effect |
|---|---|
| `Slider::new(signal, min, max)` | Bind a `Signal<f32>` + value range. |
| `.step(f32)` | Quantize movement to a step. |
| `.width(f32)` | Track width in points. |
| `.label(text)` | Accessible name. |

## Accessibility

- **Role:** `Slider`.
- **Value:** the current value is reported as the node's value string.
- **Actions:** exposes **Increment / Decrement**, so a screen reader can change
  the value directly.
- **Keyboard:** focusable; **arrow keys** adjust the value (by the step).
