//! IME preedit-overlay regression for `TextArea`.
//!
//! `paint` composes the committed text + active preedit into a *display* string,
//! re-shapes it, and re-wraps it so the candidate text renders inline (an empty
//! TextArea showed nothing while composing CJK before this). There is no
//! headless GPU paint harness, so these tests drive the **same shaping pipeline
//! paint uses** — `TextSystem::shape_document` + `slate_text::wrap_document` over
//! the composed string — and assert the geometric facts paint relies on:
//! composing widens the layout, a long composition wraps onto a new visual line,
//! and the caret lands inside the composed run. The pure splice / per-line
//! geometry math (`compose_display`, `preedit_runs`) is covered by unit tests in
//! `elements::text_area::layout`.
//!
//! Cross-platform: shaping needs only the platform text backend (CoreText /
//! DirectWrite), no window or GPU device.
#![cfg(any(target_os = "macos", target_os = "windows"))]

use slate_framework::text_system::TextSystem;

/// Splice a preedit into committed text at `caret`, exactly as
/// `text_area::layout::compose_display` does (replicated here because that
/// helper is crate-private). Keeps the test honest about what paint composes.
fn compose(committed: &str, caret: usize, preedit: &str) -> String {
    let mut at = caret.min(committed.len());
    while at > 0 && !committed.is_char_boundary(at) {
        at -= 1;
    }
    format!("{}{}{}", &committed[..at], preedit, &committed[at..])
}

fn text_system() -> (TextSystem, slate_framework::text_system::PlatformFont) {
    let mut ts = TextSystem::new().expect("create TextSystem");
    let font = ts
        .load_font_from_bytes(slate_text::TEST_FONT, 14.0, 1.0)
        .expect("load bundled font");
    (ts, font)
}

fn wrapped(ts: &TextSystem, font: &slate_framework::text_system::PlatformFont, text: &str, w: f32) -> slate_text::MultilineLayout {
    let doc = ts.shape_document(font, text).expect("shape document");
    slate_text::wrap_document(&doc, w)
}

#[test]
fn preedit_composes_into_layout_and_widens_line() {
    let (ts, font) = text_system();
    // Committed "ab", caret at end; compose CJK preedit. Generous width → 1 line.
    let committed = wrapped(&ts, &font, "ab", 1000.0);
    let display = compose("ab", 2, "你好");
    assert_eq!(display, "ab你好");
    let composed = wrapped(&ts, &font, &display, 1000.0);

    assert_eq!(composed.lines.len(), 1, "short composition stays on one line");
    let committed_w = committed.lines[0].line.width_lpx;
    let composed_w = composed.lines[0].line.width_lpx;
    assert!(
        composed_w > committed_w,
        "composed line ({composed_w}) must be wider than committed ({committed_w}) — preedit adds advance"
    );
}

#[test]
fn caret_sits_after_composition() {
    let (ts, font) = text_system();
    // cursor_byte_offset at end of the 6-byte preedit → display caret = 2 + 6.
    let display = compose("ab", 2, "你好");
    let composed = wrapped(&ts, &font, &display, 1000.0);
    let display_caret = 2 + "你好".len();

    let (line_idx, caret_x, _) = composed.caret_position(display_caret);
    assert_eq!(line_idx, 0);
    // Caret after the whole composed run sits at (or past) the committed-only
    // caret-x, and strictly inside the composed line width.
    let committed = wrapped(&ts, &font, "ab", 1000.0);
    let committed_end_x = committed.caret_position(2).1;
    assert!(
        caret_x > committed_end_x,
        "caret after composition ({caret_x}) must be past committed end ({committed_end_x})"
    );
    assert!(caret_x <= composed.lines[0].line.width_lpx + 0.01);
}

#[test]
fn long_preedit_wraps_to_next_visual_line() {
    let (ts, font) = text_system();
    // Fill a line, then compose a preedit at the end. Width tight enough that the
    // committed text alone is one line but the composition pushes onto line two.
    let committed_text = "aaaaaa";
    let one_line = wrapped(&ts, &font, committed_text, 1000.0);
    assert_eq!(one_line.lines.len(), 1, "committed text alone is one line");
    let line_w = one_line.lines[0].line.width_lpx;

    // Compose a long preedit; wrap at a width that fits the committed run but not
    // the committed run + composition.
    let display = compose(committed_text, committed_text.len(), "bbbbbbbbbb");
    let composed = wrapped(&ts, &font, &display, line_w + 1.0);
    assert!(
        composed.lines.len() > 1,
        "long composition past the wrap width must grow the line count (got {})",
        composed.lines.len()
    );
}

#[test]
fn empty_preedit_leaves_layout_unchanged() {
    let (ts, font) = text_system();
    let committed = wrapped(&ts, &font, "hello", 1000.0);
    let display = compose("hello", 3, "");
    let composed = wrapped(&ts, &font, &display, 1000.0);
    assert_eq!(display, "hello");
    assert_eq!(composed.lines.len(), committed.lines.len());
    assert!((composed.lines[0].line.width_lpx - committed.lines[0].line.width_lpx).abs() < 0.01);
}
