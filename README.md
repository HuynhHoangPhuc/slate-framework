# slate-framework

GPU-accelerated Rust UI framework. Native window/event-loop layer (no winit), `wgpu` rendering, custom WGSL SDF shaders.

**Status:** Phase 1 complete — scene data structure, four instanced pipelines (rect, image, glyph, shadow), atlas module, painter's-algorithm layer stack, 62 tests passing.

## Workspace

| Crate | Purpose |
|---|---|
| `slate-platform` | Native window + event loop (macOS via `objc2-app-kit`, Windows via `windows-rs`) |
| `slate-renderer` | `wgpu` integration, WGSL shaders, four instanced pipelines + atlas module |
| `slate-framework` | Umbrella crate; re-exports public API |
| `examples/hello-rect` | Anchor demo: opens a window and draws a rounded rect |
| `examples/primitive-gallery` | Phase 1 showcase: 150+ procedurally-generated primitives (checkerboard + dot-matrix glyphs) across 2 layers with FPS overlay |

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
