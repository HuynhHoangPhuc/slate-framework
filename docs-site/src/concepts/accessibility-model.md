# Accessibility model

Accessibility is a **shipped, first-class** part of Slate, not an afterthought.
Every interactive widget emits a semantic accessibility node, and Slate carries
real screen-reader adapters on **both** desktop platforms.

> The authoritative, versioned specification is
> [`docs/a11y-contract.md`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/docs/a11y-contract.md).
> This page summarizes it; the contract governs.

## What an element exposes

Each element may return an `AccessibilityInfo` via `Element::accessibility()`.
The key fields:

| Field | Meaning |
|---|---|
| `role` | An ARIA-style `AccessibilityRole` (`Button`, `Checkbox`, `Slider`, `Grid`, `Tree`, …). |
| `label` | Accessible name (`aria-label`). |
| `value` | Current value (sliders, text inputs). |
| `is_disabled` / `is_focused` | State flags. |
| `is_selected` / `is_expanded` | For toggles, list items, tree nodes. |
| `row_index` / `column_index` / `row_count` / `column_count` | Grid cell semantics (zero-based). |

Roles come from the `AccessibilityRole` enum (31 variants, `#[non_exhaustive]`),
covering form controls, text/structure, containers, grid/table cells, and tree
nodes. A small set of actions (`Click`, `Focus`, `Increment`, `Decrement`,
`Expand`, `Collapse`, …) lets a screen reader **operate** controls, not just
announce them.

## Roles at a glance

| Widget | Role |
|---|---|
| Button, IconButton, Select trigger | `Button` |
| Checkbox | `Checkbox` (+ `is_selected`) |
| Switch | `Switch` (+ `is_selected`) |
| Slider, Splitter divider | `Slider` (+ value, Increment/Decrement) |
| TextField, TextArea | `TextInput` (+ value) |
| List | `List` → `ListItem` |
| Tree | `Tree` → `TreeItem` (+ `is_expanded`, Expand/Collapse) |
| DataGrid | `Grid` → `Row` / `ColumnHeader` / `Cell` (+ cell indices/counts) |
| BarChart, Sparkline | `Image` (+ synthesized summary) |
| Panel / Toolbar / StatusBar | `Group` (Panel labelled by its title) |
| Select / ContextMenu / MenuList | `Menu` → `MenuItem` (+ `is_selected`) |
| Tooltip | `Group` described_by a `Tooltip` node |
| ScrollView | `ScrollView` |

These match the verified per-widget roles in the contract; each widget page
restates its role and keyboard notes.

## Operability, not just announcement

Slate routes screen-reader actions back into the framework through a shared
`route_action_request` mapping: a reader's default "press" (`Action::Click`)
focuses the node and dispatches the widget's own keyboard-activation path, so
Button/Checkbox/Switch/Slider are **operable** from the screen reader.

## Platform adapters (both shipped)

- **macOS — VoiceOver.** A per-window `accesskit_macos` adapter, productionized
  with action routing, real focus-state, and a gated per-frame tree push.
  VoiceOver-validated; part of the v1 release.
- **Windows 11 — Narrator / UIA.** A per-window, non-subclassing
  `accesskit_windows` adapter (Slate's windows are created visible, so the
  non-subclassing adapter is required). **Live-Narrator-validated on Win11.**

Both adapters share the same action-routing logic, so AT behavior never diverges
between platforms. (A Linux AT-SPI adapter is deferred post-v1.)

## Authoring tips

- Give **icon-only** controls a mandatory `aria-label` (e.g.
  `IconButton::new(icon, "Reload process list")`).
- Mark decorative labels inside a roled control with `Text::presentational()` so
  the control name isn't announced twice.
- Name container widgets with their title (`Panel::new("Processes")` labels its
  `Group`) so a reader announces the group meaningfully.

For the full field reference, semver discipline, conversion functions, and
adapter internals, read
[`docs/a11y-contract.md`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/docs/a11y-contract.md).
