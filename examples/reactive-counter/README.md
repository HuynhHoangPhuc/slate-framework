# reactive-counter

Headline demo for slate's reactive runtime + focus model. Three focusable
"counter cards", each backed by its own `Signal<i32>`, rendered side-by-side.
Card 1 also has a background async task that increments it once per second to
prove the reactive subscription stays live without user input.

> Validated 2026-05-28 on Windows 11 24H2. macOS validation in flight (this wave).

## Run

```
cargo run -p reactive-counter
```

A `560 × 380` window opens with three blue cards under the header
"Focusable Cards · Tab / click / +/- / Space".

## What it shows

- **Per-element keys + focus** — `Tab` / `Shift+Tab` cycles focus through the
  three cards; clicking a card also focuses it. The focused card draws a
  blue focus ring — except Card 2, which opts out via
  `Div::focus_ring(false)` while still receiving keyboard events.
- **Signals** — each card owns its own `Signal<i32>` and the number re-renders
  whenever that signal mutates.
- **Reactive text** — `Text::new_reactive(...)` wires the signal's `get()`
  into the render closure; no manual subscription bookkeeping.
- **Background reactivity** — a `cx.background_executor()` task ticks once
  per second and increments Card 1's counter. The number updates with no
  input from the user, proving cross-thread signal propagation.
- **App-level key handler** — the last key + modifier set is echoed under the
  cards (`last key: ... — modifiers: ⇧ ⌃ ⌥ ⌘`).

## Key bindings (per focused card)

| Key | Action |
| --- | --- |
| **Tab** / **Shift+Tab** | Cycle focus through the three cards |
| **Mouse click** | Focus a card |
| **Space** / **Enter** / **+** | Increment focused card's counter |
| **Backspace** / **-** | Decrement focused card's counter |

The modifier echo line shows: `⇧` Shift, `⌃` Ctrl, `⌥` Alt/Option,
`⌘` Meta/Win.

## Try this

| Action | Gesture | Expected |
| --- | --- | --- |
| Focus cycle | press **Tab** repeatedly | ring moves Card 1 → Card 2 (no ring, still focused) → Card 3 → wrap |
| Per-card state | focus Card 3, press **Space** ×5 | only Card 3's number changes |
| Background tick | sit idle for 3s | Card 1's counter ticks up on its own |
| Click + decrement | click Card 2, press **Backspace** | Card 2's number goes negative |

## Platform notes

- **macOS** — `Cmd` reports as Meta (`⌘`).
- **Windows** — `Win` reports as Meta (`⌘`); Alt reports as Alt (`⌥`).
- Linux is not shipped in v1.

## Known limitations

- Window size is fixed at startup with a minimum of `400 × 280`; the layout
  does not currently re-flow on extreme aspect ratios.
- The background timer keeps running while the window is minimized; this is
  intentional for the demo but is not the production recommendation.
