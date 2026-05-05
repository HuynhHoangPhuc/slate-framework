# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.1] - 2026-05-05

### Added
- Initial `slate-platform` crate with macOS/Windows native windowing (no winit)
- Initial `slate-renderer` crate with `wgpu` backend and WGSL SDF shaders
- Primitive rendering: rectangles, rounded rectangles, circles, lines
- Shadow pipeline for drop shadows with configurable blur and spread
- Layer system with z-ordering, clipping, and painter's-algorithm compositing
- `hello-rect` example: anchor demo opening a window with a rounded rect
- `primitive-gallery` example: 150+ procedurally-generated primitives across 2 layers with FPS overlay

[Unreleased]: https://github.com/HuynhHoangPhuc/slate-framework/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/HuynhHoangPhuc/slate-framework/releases/tag/v0.0.1
