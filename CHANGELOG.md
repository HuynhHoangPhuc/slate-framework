# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed — live-resize ResizeBuffers churn (device-removed + blink)
- During a modal window resize drag, `handle_window_resized` now stashes the latest size in `WindowState::pending_resize` instead of reconfiguring the swapchain on every `WindowResized`; the timer-pumped `run_redraw` drains it and applies a single `Renderer::resize` before layout/paint/present. Collapses `ResizeBuffers` from hundreds/sec to ≤1 per painted tick and keeps the buffer resize adjacent to paint.
- The per-frame perf caches (font-load, shape-line) dropped frame time ~100 ms → ~6 ms, which freed the Windows message pump to process WM_SIZE at full rate during edge-drag. On a hybrid-GPU multi-monitor laptop the resulting `ResizeBuffers` storm — fragile under wgpu + DirectComposition, where `create_texture_from_hal` retains back-buffer refs — escalated repeated failures to `0x887A0005` (`DXGI_ERROR_DEVICE_REMOVED`) and showed a stale framebuffer at the new bounds (blink/tear). The old 100 ms frame had been the accidental throttle masking it.
- Stays inside the deliberately timer-pumped resize model — does **not** revive the reverted Windows sync-resize / WM_NCCALCSIZE path. `win_compose.rs` configure logic is unchanged; it is simply called far less often. Non-drag resize, WM_DPICHANGED, and device-lost recovery paths are unaffected (`Renderer::resize` is idempotent and early-returns on a lost device, so a stash surviving a loss applies harmlessly post-recovery).
- macOS is unaffected: AppKit live resize already drives the synchronous `on_resize_sync` path, so the coalesced redraw apply short-circuits on the unchanged target size (idempotent no-op).
- Locked by `resize_coalesce_live_drag` (Windows + test-hooks): live-drag resizes defer, one redraw coalesces to the last size, non-live resizes still apply immediately. Recovery suites (`recovery_size_move_*`, `image_device_lost_recovery`) and perf benches unregressed.

### Fixed — per-frame font reload (dashboard interaction lag)
- `TextSystem::load_font` / `load_font_from_bytes` now memoize loaded fonts in a per-`TextSystem` `FontCache` keyed by `(source, size, scale)`. Each `Text` rebuilds every frame under the Strategy-A whole-view rebuild and so reloaded its font from scratch; on Windows that is a full TTF parse plus a stack of DirectWrite COM constructions (font file, face, set, collection, text format). A dense screen paid one full load per text node per frame — the dashboard's ~98 text nodes cost ~80 ms/frame (~10 fps), and the cost compounded as the DirectWrite factory accumulated font collections. The cache turns repeat loads into a refcount-bump clone, so only distinct fonts load (once).
- Dashboard steady-state frame drops from ~100 ms to ~6 ms in debug (layout-build phase ~80 ms → ~0.3 ms); form-controls (few text nodes) was already fast and is unchanged. The earlier `shape_line` cache addressed shaping, which was never the bottleneck — font *loading* was.
- Additive only: `load_font*` signatures unchanged; `DirectWriteFont` / `CoreTextFont` / `PlatformFont` gain `Clone` (a refcount bump, no re-parse). New `font_cache_*` accessors on `TextSystem` / `HeadlessApp` for tests. Covered by `text_font_cache` unit tests + the `bench_font_cache_reuse` headless test (warm renders: 0 real loads).

### Testing — macOS TextArea validation + cross-platform dispatch harness
- Converted the four TextArea dispatch suites (`text_area_editing`, `text_area_layout`, `text_area_nav`, `text_area_mouse`) to `harness = false` `[[test]]` targets whose `fn main` runs on the process main thread, so the real platform + a 1×1 window + `AppState` construct on macOS (AppKit `MainThreadMarker`). Dropped the `cfg(target_os = "windows")` gate; kept `required-features = ["test-hooks"]`.
- These suites now run cross-platform and exercise the real CoreText layout, NSPasteboard clipboard (multi-line round-trip, CRLF→LF), and IME commit paths on macOS — closing the prior Windows-only validation gap for shared TextArea logic.
- Fixed a Windows-centric test helper that hardcoded the Ctrl modifier; it now sends the platform command modifier (Cmd on macOS, Ctrl elsewhere) so clipboard/undo shortcuts fire on the host OS. Production shortcut routing (`is_command_modifier`) was already correct.

### Added — Phase 9c: IME composition + TextField element
- `slate-platform` `Event::ImePreedit`, `ImeCommit`, `ImeEnabled`, `ImeDisabled` for cross-platform composition events.
- `WindowImeDelegate` trait (4 query methods: `ime_caret_rect`, `ime_text`, `ime_selected_range`, `ime_marked_range`) — cache-only reads per ADR-001 amendment.
- macOS NSTextInputClient impl (setMarkedText, insertText, firstRectForCharacterRange, etc.); Windows IMM32 WM_IME_* message handling.
- Framework `ImeRegistry` per-element state (`ImeState` + `Preedit` types); `AppState::dispatch_ime_*` focused-chain bubble.
- Deferred ops: `pending_ime_ops` drained before `pending_focus_op` (Tab-during-composition commits preedit then moves focus).
- `CachedImeQuery` cell (republished at dispatch_ime_preedit + every paint endpoint); `WindowImeDelegate` reads cache only (eliminates re-entrant-borrow panics).
- New `TextField` element: single-line editable, `Signal<String>` binding, grapheme-aware caret motion, preedit underline overlay.
- `examples/ime-textfield`: Demo with reactive echo and macOS/Windows IME instructions.
- `unicode-segmentation = "1"` workspace dep.

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
