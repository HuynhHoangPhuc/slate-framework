# slate-framework

[![Crates.io](https://img.shields.io/crates/v/slate-framework.svg)](https://crates.io/crates/slate-framework)
[![Docs.rs](https://docs.rs/slate-framework/badge.svg)](https://docs.rs/slate-framework)

GPU-accelerated Rust UI framework. Native window/event-loop layer (no winit), `wgpu` rendering, custom WGSL SDF shaders.

## Overview

Umbrella crate that re-exports the public API from:
- [`slate-platform`](https://crates.io/crates/slate-platform) — native window + event loop
- [`slate-renderer`](https://crates.io/crates/slate-renderer) — wgpu GPU renderer

## Usage

```rust
use slate_framework::{slate_platform, slate_renderer};

// Platform
use slate_platform::{DefaultPlatform, Platform, WindowOptions, Event};

// Renderer
use slate_renderer::{Renderer, Scene};
```

## Requirements

- Rust 1.95+
- GPU with Vulkan, Metal, or DX12 support

## License

Dual-licensed under MIT or Apache-2.0.
