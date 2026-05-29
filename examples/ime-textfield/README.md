# ime-textfield

Headline demo for slate's IME composition support. A single editable
`TextField` bound to a reactive `Signal<String>`; an echo label below the field
re-renders whenever the value changes. Switch your OS input method to a CJK IME
and the field shows preedit composition (underlined provisional text + system
candidate window) before the commit.

> Validated 2026-05-28 on Windows 11 24H2 + macOS 15.x.

## Run

```
cargo run -p ime-textfield
```

Click the field to focus it (focus ring becomes visible). Then type.

## What it shows

- **Caret** — 1-px vertical bar at the insertion point, visible while focused.
- **IME preedit** — provisional composition text rendered at the caret with a
  1-px underline. The OS candidate window appears anchored to the caret rect
  reported by slate's `WindowImeDelegate`.
- **Commit** — Enter (Pinyin) or selecting a candidate inserts the chosen text
  into the buffer; the underline clears.
- **Reactive echo** — the `Signal<String>` propagates to the "You typed:" label
  on every commit (preedit alone does NOT mutate the signal, by design).
- **Grapheme-aware Backspace** — deletes one grapheme cluster (1 byte for ASCII,
  3 for CJK, 4 for a typical emoji).

## Enable IME — macOS

1. System Settings → Keyboard → Input Sources → "Edit…"
2. Click **+**, add one of:
   - **Chinese (Simplified)** → Pinyin – Simplified
   - **Japanese** → Hiragana
   - **Korean** → 2-Set Korean
3. Toggle with **Ctrl + Space** (or click the menubar input flag).
4. Open the demo, focus the field, type:
   - `nihao` (Pinyin) → 你好 candidate appears
   - `konnichiha` (Hiragana) → こんにちは
   - `annyeonghaseyo` (Hangul) → 안녕하세요

## Enable IME — Windows

1. Settings → Time & language → Language & region → "Add a language".
2. Add one of: Chinese (Simplified), Japanese, Korean.
3. Toggle with **Win + Space**.
4. Open the demo and type the same phrases as above. Microsoft IME's candidate
   window will appear above the caret.

## Try this

| Input method | Type | Expected commit |
| --- | --- | --- |
| Pinyin Simplified | `nihao` + Space/Enter | `你好` |
| Hiragana | `konnichiha` + Space + select kanji | `こんにちは` / `今日は` |
| 2-Set Korean | `annyeonghaseyo` | `안녕하세요` |
| ASCII | `Hello, world!` | `Hello, world!` |
| Emoji (macOS Ctrl+Cmd+Space) | pick 😀 | `😀` |

## What's NOT in this demo

Single-line `TextField` is intentionally minimal. The richer text-editing
behaviours — drag-select / Shift-arrow selection, clipboard, undo/redo,
click-to-position caret, blinking caret — are shipped in `TextArea` and
exercised by the [`textarea`](../textarea/) example.

## Known limitations

- Caret pixel position uses a char-count approximation of glyph advances;
  exact byte ↔ glyph mapping for multi-glyph Indic clusters is deferred.
- Tab cycles focus to the only focusable element (the field itself) — this
  demo is single-control; multi-control Tab behaviour is exercised by
  the [`reactive-counter`](../reactive-counter/) example.
- Linux is not shipped in v1.
