# textarea

Headline demo for slate's multi-line `TextArea`. A single editable area bound to
a reactive `Signal<String>`; text wraps at a fixed content width, the element
auto-grows in height (floored to `min_lines`), and an echo label below reports
the character + line count on every change.

> Validated 2026-05-28 on Windows 11 24H2 + macOS 15.x.

## Run

```
cargo run -p textarea
```

Click the area to focus it (focus ring becomes visible), then type. Enter
inserts a hard newline; long lines wrap at the fixed width.

## What it shows

- **Wrapping + auto-height** — text wraps at the style's content width; the area
  reserves at least `min_lines` rows and grows as you add lines.
- **Newline editing** — Enter inserts `\n`; Backspace across a line start joins
  lines.
- **Vertical caret nav** — ↑/↓ move between visual lines preserving a sticky
  column; Home/End jump to the **visual** line's edges (not the document's).
- **Mouse selection** — click places the caret, drag selects across visual
  lines, **double-click selects a word** (Unicode word boundaries),
  **triple-click selects the visual line**.
- **Clipboard** — Cmd/Ctrl + C/X/V copy / cut / paste; a multi-line paste keeps
  its newlines (CRLF normalized to LF).
- **Undo / redo** — Cmd/Ctrl + Z undo; Shift+Z (macOS) / Y (Windows/Linux) redo,
  spanning multi-line edits.
- **IME composition** — compose mid-document with a CJK IME; preedit renders at
  the caret and does not mutate the buffer until commit.
- **Blinking caret** — 1-px bar that blinks while focused.

## Try this

| Action | Gesture | Expected |
| --- | --- | --- |
| New line | type, then **Enter** | caret drops to a fresh line |
| Wrap | type past the width | line wraps without a newline char |
| Word select | **double-click** a word | the whole word highlights |
| Line select | **triple-click** a line | the whole visual line highlights |
| Multi-line copy | select across lines, **Ctrl/Cmd+C**, paste | newlines survive |
| Vertical nav | **↑/↓** through wrapped text | column stays put |

## Enable IME

Same as the `ime-textfield` example — see that demo's README for macOS
(Ctrl+Space) and Windows (Win+Space) input-source setup. Compose mid-document to
see the preedit insert at the caret on commit.

## Known limitations

- A drag started by a double/triple click extends at grapheme granularity, not
  by word/line (the initial snap is by word/line; the held drag is per-grapheme).
- Caret pixel position uses the shaped glyph advances; exact byte ↔ glyph mapping
  for multi-glyph Indic clusters is deferred.
- Linux is not shipped in this round.
