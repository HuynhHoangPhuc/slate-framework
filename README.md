# slate-framework

[![Release](https://img.shields.io/github/v/release/HuynhHoangPhuc/slate-framework)](https://github.com/HuynhHoangPhuc/slate-framework/releases)

GPU-accelerated Rust UI framework. Native window/event-loop layer (no winit), `wgpu` rendering, custom WGSL SDF shaders.

**Status:** Phase 2b complete — native text rendering (CoreText/DirectWrite), glyph cache, multi-line layout, font fallback chains, font smoothing dilation. Phase 1 foundations: scene data structure, four instanced pipelines, atlas module, layer stack.

## Workspace

| Crate | Purpose |
|---|---|
| `slate-platform` | Native window + event loop (macOS via `objc2-app-kit`, Windows via `windows-rs`) |
| `slate-renderer` | `wgpu` integration, WGSL shaders, four instanced pipelines + atlas module |
| `slate-text` | Native text shaping (CoreText/DirectWrite), glyph cache, paragraph layout, font fallback |
| `slate-framework` | Umbrella crate; re-exports public API |
| `examples/hello-rect` | Anchor demo: opens a window and draws a rounded rect |
| `examples/primitive-gallery` | Phase 2b showcase: native text rendering, multi-line layout, font fallback, light-on-dark |

## Build

Requires Rust 1.95+ (pinned via `rust-toolchain.toml`).

```bash
cargo check --workspace
cargo run -p hello-rect
```

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
