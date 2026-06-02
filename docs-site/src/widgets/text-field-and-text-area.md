# TextField & TextArea

**`TextField`** is a single-line text input; **`TextArea`** is the multi-line,
wrapping editor. Both bind a caller-owned `Signal<String>` and support
selection, clipboard (copy/cut/paste), undo/redo, IME composition, a blinking
caret, and bidirectional (LTR/RTL) text.

## TextField

```rust
use slate_framework::{TextField, TextFieldStyle, Color};

// Themed convenience (fills from theme tokens):
TextField::new(self.name.clone()).themed()

// Or an explicit style:
let field_style = TextFieldStyle {
    font_size: 18.0,
    color: Color::WHITE.into(),
    background: Some([0.15, 0.15, 0.18, 1.0]),
    caret_color: Color::WHITE.into(),
    preedit_selection_color: [0.4, 0.6, 1.0, 0.3],
    width: 480.0,
};
TextField::new(self.value.clone()).style(field_style)
```

> Sources:
> [`examples/form-controls/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/form-controls/src/main.rs)
> (themed) and
> [`examples/ime-textfield/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/ime-textfield/src/main.rs)
> (explicit style).

## TextArea

```rust
use slate_framework::{TextArea, TextAreaStyle, Color};

let area_style = TextAreaStyle {
    font_size: 18.0,
    color: Color::WHITE.into(),
    background: Some([0.15, 0.15, 0.18, 1.0]),
    caret_color: Color::WHITE.into(),
    selection_color: [0.4, 0.6, 1.0, 0.3],
    preedit_underline_color: Color::WHITE.into(),
    preedit_selection_color: [0.4, 0.6, 1.0, 0.3],
    width: 520.0,
    min_lines: 6,
};
TextArea::new(self.value.clone()).style(area_style)
```

> Source: [`examples/textarea/src/main.rs`](https://github.com/HuynhHoangPhuc/slate-framework/blob/main/examples/textarea/src/main.rs).

Both have `.themed()` (and `TextFieldStyle::themed()` / `TextAreaStyle::themed()`)
that pull fill colors from the active theme.

## Key props

| Builder | Effect |
|---|---|
| `TextField::new(signal)` / `TextArea::new(signal)` | Bind a `Signal<String>`. |
| `.themed()` | Use theme-token fill colors. |
| `.style(TextFieldStyle / TextAreaStyle)` | Explicit style struct. |
| `.a11y_label(name)` | Accessible name. |

`TextAreaStyle::min_lines` sets the minimum visible line count; `width` fixes the
content (wrap) width.

## Editing behavior

- Click to focus, then type. In `TextArea`, **Enter** inserts a hard newline and
  text wraps at the content width.
- **↑/↓** move between visual lines (sticky column); **Home/End** jump to the
  visual line edges; **←/→** walk the caret across LTR/RTL direction boundaries.
- **Cmd/Ctrl + C/X/V** copy / cut / paste; **Cmd/Ctrl + Z** undo,
  **Shift+Z** (macOS) / **Y** (Windows) redo.
- Double-click selects a word; triple-click selects the line; drag selects a
  range. CJK IME composition is supported with a visible preedit.

## Accessibility

- **Role:** `TextInput` (both TextField and TextArea use the same leaf role).
- **Value:** the current string is reported as the node value (TextArea reports
  the full multi-line content).
- **Label:** set via `.a11y_label(name)`.
- **Keyboard:** focusable; exposes the **Focus** action so a reader can reach it.
