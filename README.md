# slate-framework

GPU-accelerated Rust UI framework. Native window/event-loop layer (no winit), `wgpu` rendering, custom WGSL SDF shaders.

**Status:** Phase 0 — platform layer + renderer bootstrap. Not usable yet.

## Workspace

| Crate | Purpose |
|---|---|
| `slate-platform` | Native window + event loop (macOS via `objc2-app-kit`, Windows via `windows-rs`) |
| `slate-renderer` | `wgpu` integration, WGSL shaders, primitive rendering |
| `slate-framework` | Umbrella crate; re-exports public API |
| `examples/hello-rect` | Anchor demo: opens a window and draws a rounded rect |

## Build

Requires Rust 1.95+ (pinned via `rust-toolchain.toml`).

```bash
cargo check --workspace
cargo run --example hello-rect   # not yet functional in Phase 0
```

## License

Dual-licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
