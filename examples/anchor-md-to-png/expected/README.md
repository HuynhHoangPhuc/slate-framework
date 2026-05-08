# Expected Baseline Images

Per-platform PNG baselines for CI visual regression testing.

## Why Per-Platform?

Font rasterization differs between platforms:
- **Windows**: DirectWrite
- **macOS**: CoreText

Sub-pixel hinting, anti-aliasing, and glyph metrics vary enough that a single baseline would cause false failures. Each platform has its own baseline.

## Files

- `sample-windows.png` - Windows baseline (DirectWrite)
- `sample-macos.png` - macOS baseline (CoreText)

## Regenerating Baselines

When TextWrap, font rendering, or layout changes:

### Windows
```bash
cargo run --release -p anchor-md-to-png -- examples/anchor-md-to-png/inputs/sample.md examples/anchor-md-to-png/expected/sample-windows.png
```

### macOS
```bash
cargo run --release -p anchor-md-to-png -- examples/anchor-md-to-png/inputs/sample.md examples/anchor-md-to-png/expected/sample-macos.png
```

## CI Tolerance

The diff binary uses:
- **Per-channel tolerance**: 4 (absorbs anti-aliasing variance)
- **Threshold**: 0.1% of pixels may differ

If CI fails, inspect the uploaded `diff.png` artifact to see which pixels changed.

## Diff Binary

```bash
cargo run --release -p anchor-md-to-png --bin diff -- <actual.png> <expected.png>
```

Exit codes:
- `0` - Images match within tolerance
- `1` - Images differ beyond threshold
- `2` - Usage error or file not found
