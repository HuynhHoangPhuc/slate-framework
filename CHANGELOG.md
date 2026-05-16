# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added — Phase 9b: per-element keyboard handlers + focus management
- `Div::focusable(bool)`, `.tab_index(i32)`, `.focus_ring(bool)`, `.on_key_down(...)`, `.on_key_up(...)`, `.on_text_input(...)` builders for per-element keyboard wiring.
- `EventCtx::request_focus(id) / release_focus()` — deferred focus ops applied after the handler chain unwinds.
- `AppContext::set_focus(id) / clear_focus()` — programmatic focus moves from outside a handler.
- `FocusRegistry` (public) — Tab / Shift+Tab traversal sorted by `(tab_index ascending, registration index)`; `tab_index < 0` skipped by Tab but still focusable programmatically.
- Focused-chain bubble dispatch in `AppState::dispatch_key_down / key_up / text_input` (leaf → root → App-level), `cx.stop_propagation()` halts the bubble.
- Tab / Shift+Tab default focus shift (suppressed by `stop_propagation`).
- Mouse-down auto-focus: deepest focusable ancestor of the hit target gains focus before the handler runs.
- Framework-emitted 2px accent-blue focus ring overlay; opt-out via `Div::focus_ring(false)`.
- `reactive-counter` rebuilt as a 3-card focusable demo (click / Tab / Shift+Tab cycle; per-card `+ / - / Space / Enter / Backspace`; middle card opts out of the ring).

### Changed — App-level handler signature (breaking)
- `App::on_key_down`, `App::on_key_up`, `App::on_text_input` now take `FnMut(&Event, &mut EventCtx)` instead of `FnMut(&Event)`. The added `EventCtx` lets App-level handlers participate in `stop_propagation` and `request_focus` semantics. Migrate by adding `, _cx` to the closure parameter list.

## [0.0.2] - 2026-05-05

### Added
- Crates.io publishing in release workflow (automatic on tag push)
- README documentation for each crate (slate-platform, slate-renderer, slate-framework)
- Crate metadata: keywords and categories for discoverability

### Changed
- Updated GitHub Actions to latest versions (upload-artifact v7, download-artifact v8, gh-release v3)

## [0.0.1] - 2026-05-05

### Added
- Initial `slate-platform` crate with macOS/Windows native windowing (no winit)
- Initial `slate-renderer` crate with `wgpu` backend and WGSL SDF shaders
- Primitive rendering: rectangles, rounded rectangles, circles, lines
- Shadow pipeline for drop shadows with configurable blur and spread
- Layer system with z-ordering, clipping, and painter's-algorithm compositing
- `hello-rect` example: anchor demo opening a window with a rounded rect
- `primitive-gallery` example: 150+ procedurally-generated primitives across 2 layers with FPS overlay

[Unreleased]: https://github.com/HuynhHoangPhuc/slate-framework/compare/v0.0.2...HEAD
[0.0.2]: https://github.com/HuynhHoangPhuc/slate-framework/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/HuynhHoangPhuc/slate-framework/releases/tag/v0.0.1
