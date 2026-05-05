# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
