# two-window-counters

Phase 5 (D2c) demo: independent counter signals across multiple windows, plus
dynamic window creation from inside an event handler.

## What it shows

- `App::create_window` (pre-`run`): two windows registered before the run loop
  starts. Each window owns its own `Signal<i32>`, so `+1` / `-1` in window A
  does not change window B's displayed count (and vice versa).
- `AppContext::create_window` (during dispatch): clicking the green "Spawn
  another" button in window A opens a fresh counter window from inside the
  click handler. New windows continue to wake on reactive ticks without any
  extra wiring.
- Per-window focus is the native default — Tab inside window A cycles within
  A only; clicking window B reassigns the platform's focus to B.
- Quit affordance: Ctrl+Q (Win32) / Cmd+Q (macOS) calls
  `AppContext::request_quit` so the app exits even when the platform default
  (AppKit) would keep it alive after the last window closes. Win32 quits on
  last-window-close automatically; macOS requires the explicit chord.

## Run

```
cargo run -p two-window-counters
```

## Platform notes

- **Windows 11:** primary validation target for Phase 5. Closing every window
  exits the process (Win32 platform-default).
- **macOS:** D2c does not validate AppKit. D2d covers the macOS gate; the Cmd+Q
  affordance here is the path D2d will exercise.
