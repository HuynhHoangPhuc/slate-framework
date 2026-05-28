# slate-reactive

[![Crates.io](https://img.shields.io/crates/v/slate-reactive.svg)](https://crates.io/crates/slate-reactive)
[![Docs.rs](https://docs.rs/slate-reactive/badge.svg)](https://docs.rs/slate-reactive)

Reactive primitives (signals, memos, effects) for the [slate-framework](https://github.com/HuynhHoangPhuc/slate-framework) UI framework.

## Overview

Core reactive system with no UI dependencies:
- `Signal<T>` — reactive value container (`Send + Sync`)
- `Memo<T>` — cached derived value with lazy recomputation
- `Effect` — UI-thread side effect (`!Send`)
- `ReactiveOwner` — scope-based cleanup for `Memo` / `Effect`
- `Runtime` — dirty-bit, redraw hook, ID allocation, observer dispatch

## Usage

```rust
use slate_reactive::{Runtime, Signal, Memo};

let rt = Runtime::new();
let count = Signal::new(rt.clone(), 0_i32);
let doubled = Memo::new(rt.clone(), {
    let count = count.clone();
    move || count.get() * 2
});

count.set(5);
assert_eq!(doubled.get(), 10);
```

## Features

- `profiling` — in-process counters (signal-notify, effect-reentry); compiles out of default build

## License

Dual-licensed under MIT or Apache-2.0.
