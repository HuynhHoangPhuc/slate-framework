# Slate UI Framework

Slate is a **GPU-accelerated, native Rust UI framework**. It ships its own
window and event-loop layer (no `winit`), renders with `wgpu` and custom WGSL
SDF shaders, and shapes text with the platform's native engines
(CoreText on macOS, DirectWrite on Windows).

This site is the **adoption guide**: getting-started, the mental models you need
to be productive, and a reference page per shipped widget. For item-level API
signatures, generate the rustdoc with `cargo doc --open` — this book is the
narrative companion, not a rustdoc replacement.

## What you get

- A **reactive** UI model built on fine-grained signals (`Signal`, `Memo`,
  `Effect`) with a simple whole-view rebuild strategy ("Strategy A").
- **Flexbox layout** via [Taffy](https://github.com/DioxusLabs/taffy).
- A **theme token** system with ambient light/dark switching.
- An **overlay layer** with an anchor solver, modal scrim, and focus trapping.
- A complete **widget set** — form controls, data-display widgets, charts,
  layout containers, overlay widgets, and **native OS menus**.
- A **shipped accessibility model** with both macOS (VoiceOver) and
  Windows (Narrator/UIA) screen-reader adapters.

## How this book is organized

- **Getting started** — add the dependency, draw your first window, and run it
  on macOS and Windows.
- **Concepts** — the load-bearing ideas: signals, elements vs widgets, layout,
  theming, overlays, and accessibility.
- **Widget reference** — one page per shipped widget, each with a real snippet
  pulled from the `examples/` crates plus its accessibility notes.
- **Reference app** — the `dashboard` example, a process-monitor shell that
  exercises every widget.

## Source of truth

Every snippet in this book is excerpted from a runnable crate under
`examples/`. When in doubt, read the example: it is the canonical, compiled
form of the API. Accessibility notes are sourced from `docs/a11y-contract.md`.
