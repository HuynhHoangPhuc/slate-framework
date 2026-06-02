# Reactive signals

Slate's reactivity is built on fine-grained signals from the `slate-reactive`
crate, re-exported as `slate_framework::reactive`. The surface is small:
`Signal`, `Memo`, `Effect`, plus a `Runtime` that owns them.

## Strategy A: whole-view rebuild

Slate uses a deliberately simple update model the codebase calls **Strategy A**:

> When a signal that the view read during `render` changes, the **entire view is
> rebuilt** (`render` runs again) and the new element tree is diffed for paint.

There is no per-node reactive graph wiring in user code. You just:

1. Create state as a `Signal<T>` (once, outside `render`).
2. Read it inside `render` (or inside a reactive closure like
   `Text::new_reactive`). The read **subscribes** the view.
3. Write it from an event handler (`set`, `update`). The write triggers a
   rebuild.

This keeps app code free of manual subscription bookkeeping at the cost of
re-running `render` — cheap because the element tree is lightweight and layout
is incremental.

## Creating and using a signal

Signals are created from the runtime, reachable via `AppContext::runtime()`
inside `App::run`:

```rust
use slate_framework::reactive::Signal;

App::new(opts).run(move |cx: &AppContext| {
    let count = Signal::new(cx.runtime(), 0i32);
    // ... build your View holding `count` ...
});
```

Hold signals on the `View` struct so they **survive the rebuild** — they are
created once in the `App::run` factory, not inside `render`:

```rust
struct DemoView {
    counts: [Signal<i32>; 3],
    last_key: Signal<String>,
}
```

> Source: [`examples/reactive-counter/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/reactive-counter/src/main.rs).

## Reading and writing

```rust
// Read (subscribes the current observer — e.g. the view being rendered):
let n = counter.get();

// Write a new value:
last_key.set("Enter".to_string());

// Mutate in place:
counter.update(|n| *n += 1);
```

A reactive text node re-reads its closure whenever the signals it touches
change:

```rust
Text::new_reactive(move || c_text.get().to_string())
```

## Caller-owned widget state

Every interactive Slate widget follows the same contract: **you own the state
signal, the widget reads and writes it.** A `Checkbox` takes a `Signal<bool>`, a
`Slider` takes a `Signal<f32>`, a `Select` takes a `Signal<usize>`. Create those
signals once on your `View` so they persist across the Strategy-A rebuild. This
is why the widget pages all show a signal being passed in.

## Background work

Async tasks run on the background executor and can write signals to drive the
UI. A signal write from a spawned task triggers the same whole-view rebuild:

```rust
let bg = cx.background_executor();
let counter0 = counts[0].clone();
bg.spawn(async move {
    loop {
        Timer::after(Duration::from_secs(1)).await;
        counter0.update(|n| *n += 1);
    }
})
.detach();
```

> Source: [`examples/reactive-counter/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/reactive-counter/src/main.rs).
