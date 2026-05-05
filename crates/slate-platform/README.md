# slate-platform

[![Crates.io](https://img.shields.io/crates/v/slate-platform.svg)](https://crates.io/crates/slate-platform)
[![Docs.rs](https://docs.rs/slate-platform/badge.svg)](https://docs.rs/slate-platform)

Native window and event-loop abstraction for the [slate-framework](https://github.com/HuynhHoangPhuc/slate-framework) UI framework.

## Overview

Platform abstraction layer providing:
- Cross-platform `Platform` and `Window` traits
- Native event loop integration
- Raw window handle support (`raw-window-handle 0.6`)

**Backends:** macOS (`objc2-app-kit`), Windows (`windows-rs`). Linux planned.

## Usage

```rust
use slate_platform::{DefaultPlatform, Platform, WindowOptions, Event};

let platform = DefaultPlatform::new();
let window = platform.create_window(WindowOptions {
    title: "My App".into(),
    size: (800, 600),
    ..Default::default()
});

platform.run(|event| match event {
    Event::WindowCloseRequested { .. } => platform.quit(),
    Event::WindowRedrawRequested { .. } => { /* render */ }
    _ => {}
});
```

## License

Dual-licensed under MIT or Apache-2.0.
