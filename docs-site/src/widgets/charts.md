# BarChart & Sparkline

Two lightweight visualizations drawn with primitive quads (no dedicated renderer
pipeline). **`BarChart`** draws a set of bars; **`Sparkline`** draws a compact
trend polyline. Both take a plain `Vec<f32>` of data.

## Usage

```rust
use slate_framework::{BarChart, Sparkline};

BarChart::new(data::per_core_load()).size(180.0, 90.0).label("Per-core load")
Sparkline::new(data::cpu_history()).size(220.0, 64.0).label("CPU history")
```

> Source: [`examples/dashboard/src/panels.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/panels.rs).

The data are plain numeric series:

```rust
fn per_core_load() -> Vec<f32> {
    (0..8).map(|i| 20.0 + (i as f32 * 11.0) % 70.0).collect()
}
fn cpu_history() -> Vec<f32> {
    (0..32).map(|i| 30.0 + 25.0 * ((i as f32) * 0.5).sin()).collect()
}
```

> Source: [`examples/dashboard/src/data.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/dashboard/src/data.rs).

## Key props

| Builder | Effect |
|---|---|
| `BarChart::new(Vec<f32>)` | Bar values. |
| `Sparkline::new(Vec<f32>)` | Trend points. |
| `.size(width, height)` | Chart size in points. |
| `.label(name)` | Accessible summary name. |

A `polyline_quads(points, stroke_width)` helper is also public for building
custom polyline-based visualizations on the same rect pipeline.

## Accessibility

- **Role:** `Image`.
- **Summary:** the chart synthesizes a textual summary (e.g. a bar count and the
  value range) so a screen reader announces something meaningful in place of the
  pixels. Give a `.label(..)` to name the chart.
