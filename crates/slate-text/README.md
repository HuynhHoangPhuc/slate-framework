# slate-text

[![Crates.io](https://img.shields.io/crates/v/slate-text.svg)](https://crates.io/crates/slate-text)
[![Docs.rs](https://docs.rs/slate-text/badge.svg)](https://docs.rs/slate-text)

Native text shaping and rasterization for the [slate-framework](https://github.com/HuynhHoangPhuc/slate-framework) UI framework.

## Overview

Platform-native text rendering feeding glyphs into `slate-renderer`'s `GlyphPipeline`:
- Single-line and multi-line text with greedy word wrap
- Font fallback chains with system font enumeration
- 4 sub-pixel X variants for glyph positioning
- Font smoothing dilation (macOS) using BT.709 luminance weights
- Line-layout cache with two-frame rolling eviction

**Backends:** macOS (CoreText), Windows (DirectWrite).

## Unit Convention

All measurements use **logical pixels (lpx)** as the canonical unit:
- 1 lpx = 1 DIP at scale=1.0 = 1 point × 96/72
- CoreText (point-based) converts at font load
- DirectWrite (DIP-native) is 1:1 with logical pixels

## Bundled Fonts

See `LICENSES/` in the workspace root for licensing of any fonts bundled in tests or examples.

## License

Dual-licensed under MIT or Apache-2.0.
