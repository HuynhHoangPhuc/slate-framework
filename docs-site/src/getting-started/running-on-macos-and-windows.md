# Running on macOS & Windows

Slate targets macOS and Windows 11 from the same source. The window/event-loop
layer is native per platform (macOS via `objc2-app-kit`, Windows via
`windows-rs`); your `View` code is identical across both.

## Run an example

The repository ships runnable examples. From the workspace root:

```bash
# The minimal element demo
cargo run -p hello-element

# A reactive, keyboard-focusable counter
cargo run -p reactive-counter

# The full reference app (process monitor)
cargo run -p dashboard
```

Each example crate names its run command in the header comment of its
`src/main.rs`.

## Logging

Examples call `env_logger::init()` at the top of `main`, so you can surface
framework and app logs with the `RUST_LOG` environment variable:

```bash
# macOS / Linux shell
RUST_LOG=info cargo run -p dashboard
```

```powershell
# Windows PowerShell
$env:RUST_LOG = "info"; cargo run -p dashboard
```

## Platform notes

- **macOS** — text is shaped with CoreText; the accessibility adapter wires into
  **VoiceOver**. The window's Metal view is the first responder, so keyboard and
  IME flow without extra setup.
- **Windows 11** — text is shaped with DirectWrite; the accessibility adapter
  wires into **Narrator** over UIA. Slate creates its Win32 windows already
  visible and uses a non-subclassing UIA adapter accordingly.
- **Native menus** are real OS menus on both platforms: an `NSMenu` bar on
  macOS, an `HMENU` on Windows. The same [`Menu`](../widgets/native-menus.md)
  model drives both — see the [native menus](../widgets/native-menus.md) page.
- **Window geometry** can persist across restarts by installing a
  `PersistenceStore` and giving a window a `persistence_key`; the dashboard
  demonstrates a file-backed store.

## Build the whole workspace

```bash
cargo check --workspace
```
