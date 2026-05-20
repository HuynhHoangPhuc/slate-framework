//! Integration smoke test for the multi-line `TextArea` element.
//!
//! There is no cross-platform render harness (`test_support.rs` covers GPU
//! recovery only), so `prepaint`/`paint` cannot run here — the layout caching
//! onto `ImeState` happens inside `paint` and is exercised manually by the
//! framework at runtime. What this test *can* verify without a GPU is the two
//! halves TextArea is built from:
//!
//!   1. the element's public API constructs (builder + style chain), and
//!   2. the real Windows DirectWrite layout path produces the expected
//!      visual-line model — a 3-row hard-newline document wraps to 3 lines and
//!      the document-start caret resolves to line 0.
//!
//! Windows-only for the same reason as the other dispatch tests: the live
//! platform text backend.

#![cfg(all(target_os = "windows", feature = "test-hooks"))]

use slate_framework::elements::{TextArea, TextAreaStyle};
use slate_framework::text_system::TextSystem;
use slate_reactive::{Runtime, Signal};

// ---------------------------------------------------------------------------
// Smoke: TextArea public API compiles (no view tree required)
// ---------------------------------------------------------------------------

#[test]
fn text_area_module_smoke() {
    let rt = Runtime::new();
    let signal = Signal::new(rt, String::new());
    let style = TextAreaStyle {
        width: 320.0,
        min_lines: 3,
        ..Default::default()
    };
    // Builder chain + custom style compile and bind to the signal.
    let _ta = TextArea::new(signal).style(style);
}

// ---------------------------------------------------------------------------
// Layout: hard newlines wrap to one visual line each; caret-at-start → line 0
// ---------------------------------------------------------------------------

#[test]
fn three_line_document_wraps_to_three_lines_with_caret_on_line_zero() {
    // Real platform text system — shaping only, no GPU device required.
    let mut text_system = TextSystem::new().expect("create TextSystem");
    let font = text_system
        .load_font_from_bytes(slate_text::TEST_FONT, 14.0, 1.0)
        .expect("load bundled font");

    // Three hard-newline-separated paragraphs, each short enough to occupy a
    // single visual line at a generous width.
    let doc = text_system
        .shape_document(&font, "alpha\nbeta\ngamma")
        .expect("shape document");
    let layout = slate_text::wrap_document(&doc, 1000.0);

    assert_eq!(
        layout.lines.len(),
        3,
        "two hard newlines must yield three visual lines"
    );

    // Byte ranges tile 0..text.len() with no gaps.
    assert_eq!(layout.lines[0].byte_start, 0);
    for pair in layout.lines.windows(2) {
        assert_eq!(
            pair[0].byte_end, pair[1].byte_start,
            "visual-line byte ranges must be contiguous"
        );
    }
    assert_eq!(
        layout.lines.last().unwrap().byte_end,
        "alpha\nbeta\ngamma".len(),
        "final line must cover to text end"
    );

    // The document-start caret (focused-field initial position) lands on line 0
    // at the line's left edge.
    let (line_idx, x_lpx, y_lpx) = layout.caret_position(0);
    assert_eq!(line_idx, 0, "caret at byte 0 resolves to the first line");
    assert_eq!(x_lpx, 0.0, "caret at byte 0 sits at the line's left edge");
    assert_eq!(y_lpx, 0.0, "first line's top is at y = 0");

    // Vertical layout is monotonic top-to-bottom by a uniform line height.
    assert!(layout.line_height_lpx > 0.0);
    assert_eq!(layout.lines[1].line.y_offset_lpx, layout.line_height_lpx);
    assert_eq!(
        layout.lines[2].line.y_offset_lpx,
        2.0 * layout.line_height_lpx
    );
}

// ---------------------------------------------------------------------------
// Multi-space: the word-wrap path (TextArea) preserves every ASCII space, so
// caret-x advances once per space byte instead of collapsing the run.
// ---------------------------------------------------------------------------

#[test]
fn multi_space_run_preserves_every_space_byte() {
    let mut text_system = TextSystem::new().expect("create TextSystem");
    let font = text_system
        .load_font_from_bytes(slate_text::TEST_FONT, 14.0, 1.0)
        .expect("load bundled font");

    // Single space vs five spaces between the same two words: the 5-space run
    // must be ~4 extra space-advances wider (not collapsed to one).
    let one = text_system.shape_document(&font, "a b").expect("shape one");
    let five = text_system.shape_document(&font, "a     b").expect("shape five");
    let one_w = slate_text::wrap_document(&one, 1000.0).lines[0].line.width_lpx;
    let five_w = slate_text::wrap_document(&five, 1000.0).lines[0].line.width_lpx;
    assert!(
        five_w > one_w + 1.0,
        "5-space run must be wider than 1-space ({five_w} vs {one_w})"
    );

    // Caret-x is strictly increasing across each space byte of "a     b"
    // (bytes 1..=6 are the run interior + the following 'b').
    let layout = slate_text::wrap_document(&five, 1000.0);
    let mut prev_x = layout.caret_position(1).1; // just after 'a'
    for byte in 2..=6 {
        let x = layout.caret_position(byte).1;
        assert!(
            x > prev_x,
            "caret-x must advance at space byte {byte}: {x} !> {prev_x}"
        );
        prev_x = x;
    }
}

// ---------------------------------------------------------------------------
// Regression guard for the TextField path: TextField shapes its whole display
// string via `shape_line` (not the word-wrap path), which already preserves
// spaces. Lock that in so the TextArea fix never tempts a shared regression.
// ---------------------------------------------------------------------------

#[test]
fn text_field_shape_line_preserves_spaces() {
    let mut text_system = TextSystem::new().expect("create TextSystem");
    let font = text_system
        .load_font_from_bytes(slate_text::TEST_FONT, 14.0, 1.0)
        .expect("load bundled font");

    let one = text_system.shape_line(&font, "a b").expect("shape one");
    let five = text_system.shape_line(&font, "a     b").expect("shape five");
    assert!(
        five.width_lpx > one.width_lpx + 1.0,
        "shape_line must keep all 5 spaces ({} vs {})",
        five.width_lpx,
        one.width_lpx
    );
}
