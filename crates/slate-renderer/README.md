# slate-renderer

[![Crates.io](https://img.shields.io/crates/v/slate-renderer.svg)](https://crates.io/crates/slate-renderer)
[![Docs.rs](https://docs.rs/slate-renderer/badge.svg)](https://docs.rs/slate-renderer)

wgpu-based GPU renderer for the [slate-framework](https://github.com/HuynhHoangPhuc/slate-framework) UI framework.

## Overview

GPU rendering backend providing:
- Four instanced pipelines: rect, image, glyph, shadow
- Atlas module for texture packing
- Scene graph with painter's-algorithm layer stack
- Custom WGSL SDF shaders

Built on `wgpu` — Metal on macOS, DX12 on Windows.

## Usage

```rust
use slate_renderer::{Renderer, Scene, Layer, RectInstance};

// Create renderer inside Event::Resumed handler
let renderer = pollster::block_on(Renderer::new(window.clone()))?;

// Build scene
let mut scene = Scene::new();
scene.push_layer(Layer::new());
scene.push_rect(RectInstance { /* ... */ });

// Render
renderer.render_scene(&mut scene)?;
```

**Important:** `Renderer::new` must be called from inside `Event::Resumed` — the OS run loop must be alive for surface creation.

## License

Dual-licensed under MIT or Apache-2.0.
