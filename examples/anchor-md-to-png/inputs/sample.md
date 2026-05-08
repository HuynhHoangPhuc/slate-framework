# Slate Framework

A GPU-accelerated Rust UI framework built on wgpu.

## Features

- **Three-phase Element lifecycle** for predictable rendering
- **Taffy-based layout** with CSS Flexbox and Grid support
- **Async runtime** with smol executor
- **Accessibility hooks** built in from day one

## Getting Started

Here's a simple example:

```rust
let ui = Div::new()
    .background(Color::from_hex("#1a1a2e"))
    .padding(Edges::all(16.0))
    .child(Text::new("Hello, Slate!"));
```

## Architecture

> The framework follows GPUI's proven three-phase Element lifecycle:
> request_layout, prepaint, and paint.

### Core Components

- Element trait with type-erased AnyElement
- LayoutTree backed by Taffy
- TextSystem for font loading and shaping
- HitTestList for input handling

## Next Steps

- Phase 4: Signals and reactivity
- Phase 5: Full event delivery
- Phase 6: Platform accessibility integration
